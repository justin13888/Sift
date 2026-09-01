import AppKit
import AuthenticationServices
import CSift

/// The account-less state, which **is** the add-account flow rather than an empty inbox.
///
/// An empty inbox with a hint in it tells a new user the product is broken. So first run is a
/// screen, and this is it: what Sift is, what it will not do, what it is about to look up, and
/// then the handoff to the browser.
///
/// # The three things this screen has to say before anything else
///
/// **There is no send path.** Not a limitation disclosed later — it is the shape of the
/// product, and a user who discovers it after connecting their mail has been misled by
/// omission. It is the first sentence on the screen.
///
/// **What Sift is about to look up, before it looks anything up.** FR-3 requires the discovery
/// steps be disclosed in order, with the one that leaves the user's machine — the public
/// provider database — carrying its own control to decline it. A disclosure shown after the
/// lookup is a notification, not a disclosure.
///
/// **That the account starts read-only.** Sift will sync and show the mail and change nothing
/// in it until the user says otherwise, and saying that up front is what makes the first sync
/// against a real mailbox something a person can watch without anxiety.
final class AddAccountWindow: NSWindowController {
    private let app: OpaquePointer
    private let onAdded: () -> Void
    private let status = NSTextField(labelWithString: "")
    private var pending: String?

    /// The OAuth client this build was configured with, from the bundle.
    ///
    /// Absent is a legitimate state and is stated rather than hidden: a build with no client
    /// runs against the recorded corpus, which is how Sift is meant to be looked at before a
    /// real mailbox is connected.
    private static var clientID: String? {
        let value = ApplicationShell.configuredClientID
        return value.isEmpty ? nil : value
    }

    /// Held for the life of the flow, because the session is cancelled when it is released.
    private var session: ASWebAuthenticationSession?

    init(app: OpaquePointer, onAdded: @escaping () -> Void) {
        self.app = app
        self.onAdded = onAdded
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 620, height: 560),
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered, defer: false)
        window.title = "Add an Account"
        window.isReleasedWhenClosed = false
        super.init(window: window)
        window.contentView = build()
        window.center()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    private func build() -> NSView {
        let heading = NSTextField(labelWithString: "Sift reads your mail. It does not send it.")
        heading.font = .preferredFont(forTextStyle: .title2)
        heading.lineBreakMode = .byWordWrapping
        heading.preferredMaxLayoutWidth = 560

        let body = NSTextField(
            wrappingLabelWithString: """
                There is no compose window, and Sift never connects to an outgoing mail server. \
                Reply and Forward hand the message to your usual mail app.

                Sift will read, search and file your mail, and it starts out only watching: it \
                syncs and shows everything, and changes nothing in your mailbox until you say \
                so. Archive, flag and delete are recorded here and held until then.
                """)
        body.font = .preferredFont(forTextStyle: .body)
        body.textColor = .secondaryLabelColor
        body.preferredMaxLayoutWidth = 560

        // FR-3, in order, and before anything runs. The step that leaves the machine is marked
        // as such and can be declined on its own — a disclosure the user cannot act on is a
        // notice rather than a disclosure.
        let disclosure = NSTextField(
            wrappingLabelWithString: """
                Before connecting, Sift looks for your provider in this order:

                    1.  a setting you type in yourself — nothing leaves your machine
                    2.  the well-known configuration path on your own mail domain
                    3.  your domain's published mail records
                    4.  a public database of provider settings — this one tells a third \
                party which domain you use

                You can skip the last step and enter the settings by hand.
                """)
        disclosure.font = .preferredFont(forTextStyle: .callout)
        disclosure.preferredMaxLayoutWidth = 560

        status.font = .preferredFont(forTextStyle: .callout)
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byWordWrapping
        status.preferredMaxLayoutWidth = 560

        let connect = NSButton(
            title: "Connect a Google Account…", target: self, action: #selector(connect))
        connect.bezelStyle = .rounded
        connect.keyEquivalent = "\r"

        let fixtures = NSButton(
            title: "Look Around First", target: self, action: #selector(useFixtures))
        fixtures.bezelStyle = .rounded

        if AddAccountWindow.clientID == nil {
            connect.isEnabled = false
            status.stringValue = """
                This build has no OAuth client configured, so it cannot connect to a real \
                account. "Look Around First" opens Sift with a recorded mailbox — no network, \
                no credentials, and nobody's mail in it.
                """
        } else {
            status.stringValue =
                "Sift will open your browser to sign in. The reply comes back to Sift directly."
        }

        let buttons = NSStackView(views: [fixtures, connect])
        buttons.orientation = .horizontal
        buttons.spacing = 12

        let stack = NSStackView(views: [heading, body, disclosure, status, buttons])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 18
        stack.edgeInsets = NSEdgeInsets(top: 28, left: 30, bottom: 28, right: 30)
        stack.translatesAutoresizingMaskIntoConstraints = false

        let container = NSView()
        container.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: container.topAnchor),
            stack.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            stack.bottomAnchor.constraint(lessThanOrEqualTo: container.bottomAnchor),
        ])
        return container
    }

    @objc private func connect() {
        guard let client = AddAccountWindow.clientID else { return }

        // The scheme the callback will arrive on, derived by the layer from the configured
        // client. Not derived here: D-17 exists to stop the two shells growing two answers to
        // which scheme a provider accepts, and this one is not the obvious answer — the client
        // identifier reversed, for the only client type compatible with NFR-24.
        var schemeOut = SiftStr()
        guard sift_callback_scheme(UnsafeMutablePointer(app), &schemeOut) == Ok else { return }
        let scheme = SiftText.string(schemeOut)

        var url = SiftStr()
        let began = sift_begin_authorization(UnsafeMutablePointer(app), &url) == Ok
        guard began, let address = URL(string: SiftText.string(url)) else {
            // D-36 and D-71: the layer refuses to begin where the bundle does not claim the
            // scheme this client requires, and it refuses *here*, before the user goes
            // anywhere. Coming back from a browser to nothing is the failure this ordering
            // exists to prevent, and it is the failure that happened when this shell asserted
            // the registration instead of reporting what its bundle actually claims.
            let claimed = ApplicationShell.claimedURLSchemes()
            status.stringValue = """
                Sift did not start the sign-in, because this build cannot receive the reply. \
                The client it was configured with returns through \u{201C}\(scheme)\u{201D}, and this \
                bundle registers \(claimed.isEmpty ? "no schemes at all" : claimed.joined(separator: ", ")). \
                Rebuild with `mise run macos`, which now refuses to produce a bundle that \
                disagrees with its own client.
                """
            return
        }

        pending = client
        status.stringValue = "Waiting for your browser…"

        // **The reply comes back to this process, not through the operating system.**
        // `ASWebAuthenticationSession` is the platform's own API for a redirect into a scheme
        // an application owns: the same system browser, sharing its cookies — so FR-2's reason
        // for refusing an embedded view holds, since Sift never sees the password — and the
        // callback URL is handed straight to this closure.
        //
        // What that removes is the hand-back: a browser following a server redirect into a
        // custom scheme, LaunchServices resolving it, and Sift being alive to receive it. Each
        // of those is a way for a completed consent to arrive nowhere, and the last one cannot
        // succeed at all, because the PKCE verifier lives in the process that began the flow.
        // D-36 is unchanged in what it declares — the redirect is still the registered scheme,
        // never a loopback, and NFR-24 still admits no socket. Only the return route is the
        // platform's rather than the operating system's.
        let session = ASWebAuthenticationSession(url: address, callbackURLScheme: scheme) {
            [weak self] callback, error in
            guard let self else { return }
            self.session = nil
            guard let callback else {
                // A user who closed the browser said no. It must not present as a failure.
                if let error = error as? ASWebAuthenticationSessionError,
                    error.code == .canceledLogin
                {
                    self.status.stringValue = "Sign-in was cancelled. You can try again."
                } else {
                    self.status.stringValue = "That sign-in did not complete. You can try again."
                }
                return
            }
            self.callbackArrived(callback.absoluteString)
        }
        session.presentationContextProvider = self
        // Not ephemeral, which is the default and is stated because it is a decision: the
        // session shares the browser's cookies, so a user already signed in to the provider is
        // not made to sign in again. It is unrelated to consent — the profile asks for that
        // every time with `prompt=consent`, which is what makes a refresh token arrive on a
        // re-add rather than only on the first one.
        session.prefersEphemeralWebBrowserSession = false
        self.session = session
        guard session.start() else {
            status.stringValue = "Sift could not open a browser for the sign-in."
            self.session = nil
            return
        }
    }

    /// The callback, from the authentication session or from the registered URI scheme.
    ///
    /// **Not a socket.** NFR-24 admits none for any purpose. The session above is the route a
    /// sign-in started here takes; this stays reachable from `application(_:open:)` because the
    /// bundle still registers the scheme under D-109 and a callback arriving that way should be
    /// answered rather than dropped.
    func callbackArrived(_ url: String) {
        var id = SiftId.zero
        let name = "Mail"
        let ok = SiftText.withBytes(url) { urlPtr, urlLen in
            SiftText.withBytes(name) { namePtr, nameLen in
                sift_complete_authorization(
                    UnsafeMutablePointer(app), urlPtr, urlLen, namePtr, nameLen, &id) == Ok
            }
        }
        guard ok else {
            // A callback whose state matches no flow in progress is discarded without comment
            // at the layer — any local process can invoke a registered scheme, so a message
            // here would tell an attacker their guess was received.
            status.stringValue = "That sign-in did not complete. You can try again."
            return
        }
        close()
        onAdded()
    }

    @objc private func useFixtures() {
        var id = SiftId.zero
        let label = "fixtures"
        let added = SiftText.withBytes(label) { ptr, len in
            sift_add_replayed_account(UnsafeMutablePointer(app), ptr, len, &id) == Ok
        }
        guard added else {
            status.stringValue = "The recorded mailbox could not be opened."
            return
        }
        _ = SiftText.withBytes(label) { ptr, len in
            sift_sync_account(UnsafeMutablePointer(app), ptr, len)
        }
        close()
        onAdded()
    }
}


// MARK: - Where the authentication session puts its window

extension AddAccountWindow: ASWebAuthenticationPresentationContextProviding {
    /// Sift launches as an accessory, and an accessory presenting a sheet on no window puts it
    /// somewhere the user cannot reach. This screen always has a window, and it is the right
    /// one: the sign-in belongs to the account being added.
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        window ?? NSApp.keyWindow ?? NSWindow()
    }
}
