import AppKit
import CSift
import WebKit

/// D-3's body view: message HTML in its own document, with **no network capability at all**.
///
/// # N-1, in WebKit terms
///
/// The invariant is that only the internal scheme is registered and every other scheme is
/// rejected at the engine's policy layer. Five things enforce it here:
///
/// 1. A `WKURLSchemeHandler` is registered for `sift-resource` and nothing else. WebKit will
///    not let a page load a scheme it has no handler for and no built-in support for.
/// 2. **A content rule list blocks every subresource load whose scheme is not the internal
///    one**, compiled once per process and installed before the first document is shown
///    ([`BodyViewIsolation`]). This is the policy layer for http, https, websockets and blob:
///    WebKit has built-in support for all four, so a missing handler does not stop them, and
///    the navigation callback in 3 never sees a subresource. A body view whose rule list failed
///    to compile **renders nothing** rather than rendering without it.
/// 3. `decidePolicyFor` admits exactly one navigation per document — the main-frame load this
///    view itself started, at an address minted for that load and consumed by it — and cancels
///    every other. Subframes, refreshes, reloads, and links never proceed in place.
/// 4. The data store is **non-persistent** and minted per view, so two views share no cookie
///    jar, no cache and no local storage — NFR-25's correlation channel, closed.
/// 5. A `default-src 'none'` content security policy is injected as the first element of the
///    document. It is a backstop for everything the other four cover, and — measured by the P0
///    probe (`BodyViewProbe`) rather than assumed — **the only engine layer that refuses a
///    `data:` or `file:` subresource**: content rule lists do not match `data:`, and WebKit
///    resolves both schemes internally without consulting a registered handler. A `data:`
///    resource is bytes the document already carries rather than egress, and the sanitizer
///    rewrites every fetching position before the document gets here; the probe holds the CSP
///    to refusing them anyway, because a backstop nobody checks is not one.
///
/// # Script is off at the engine, not at the page — D-50
///
/// WebKit offers both settings and NFR-20's words are satisfied by either. The narrow one
/// disables script that arrives with the document and leaves host-injected evaluation running;
/// the wide one disables the engine's script support outright. This uses the wide one, because
/// the sanitizer invariants list "JavaScript disabled at the engine level" as I1's
/// *independent* backstop — and that sentence is only true under the wide setting. Under the
/// narrow one the backstop is a policy about where script came from, enforced by the same
/// engine whose parser disagreeing with Sift's is the entire mutation-XSS class it exists to
/// catch.
///
/// # Content height, without script
///
/// D-50 removes the obvious route: there is no `evaluateJavaScript` here to ask the document
/// how tall it is. So the body view **owns its own scrolling** rather than participating in an
/// outer one, and never needs to know. That is compatible with D-54's native rows above one
/// body view.
///
/// # The layout width is pinned — Q-15
///
/// The sanitized document keeps the sender's media queries, and the engine evaluates them
/// against its own viewport. The dark transform resolved those same queries once, in the core,
/// at one width — so a web view that reflowed with the reader pane would apply the sender's
/// colours for one width underneath overrides computed for another, and step 5's contrast
/// repair would describe a pair that no longer exists. The web view is therefore **always
/// exactly `layoutWidth` points wide**, whatever the pane does: centred when the pane is wider,
/// scrolled sideways when it is narrower. Resizing the reader moves the column; it never
/// reflows the document.
final class BodyView: NSView {
    /// The width every body is laid out at. **This is the cascade's pinned viewport** —
    /// `sift_css::cascade::Viewport::default()` in the core — and the two must agree, or the
    /// engine evaluates the sender's media queries at a width the transform never saw.
    static let layoutWidth: CGFloat = 800

    private var web: WKWebView!
    /// Carries the pinned column sideways when the pane is narrower than it. Vertical
    /// scrolling stays the web view's own, as it was before the width was pinned.
    private let scroller = NSScrollView()
    private let column = NSView()
    private var app: OpaquePointer?
    /// The document currently loaded. Revoked before the next one is opened — D-90.
    private var token: String?

    /// The one navigation this view will admit: the main-frame load it started itself, at an
    /// address minted for that load. Consumed by the policy decision that admits it, so a
    /// refresh, a reload, or a subframe naming the same address is cancelled like any other.
    private var admitted: URL?
    /// Whether [`BodyViewIsolation`]'s rule list is installed. Nothing is loaded before it is.
    private var isolated = false
    /// The document waiting for the rule list, if `show` arrived first. Only the latest is
    /// kept: an earlier one was superseded before it could be shown.
    private var pending: String?

    /// The P0 probe's recorder, when this view is being measured rather than read in.
    /// Always `nil` in the application: see [`BodyViewInstrument`].
    private let instrument: BodyViewInstrument?

    /// Told when a link is activated, so the confirmation sheet can be raised.
    var onLink: ((URL) -> Void)?

    override convenience init(frame: NSRect) {
        self.init(frame: frame, instrument: nil)
    }

    /// A body view whose every attempted load, navigation and script execution is reported
    /// to `instrument`. The configuration is otherwise the application's own, byte for byte,
    /// which is the point: the probe measures the view that ships, not a copy of it.
    init(frame: NSRect, instrument: BodyViewInstrument?) {
        self.instrument = instrument
        super.init(frame: frame)

        let configuration = WKWebViewConfiguration()

        // NFR-25: its own store, non-persistent, minted for this view. Two messages cannot
        // correlate through it and neither can observe application state.
        configuration.websiteDataStore = .nonPersistent()

        // D-50, the wide setting. `allowsContentJavaScript` alone would be the narrow one.
        configuration.defaultWebpagePreferences.allowsContentJavaScript = false
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.preferences.isFraudulentWebsiteWarningEnabled = false
        configuration.preferences.setValue(false, forKey: "javaScriptEnabled")
        // Nothing in a body plays itself; media that could would be a fetch nobody asked for.
        configuration.mediaTypesRequiringUserActionForPlayback = .all

        // N-1's one registered scheme. Everything else has no handler here, and whatever the
        // engine supports natively is refused by the rule list installed below.
        configuration.setURLSchemeHandler(
            ResourceSchemeHandler(
                app: { [weak self] _ in self?.app },
                observe: instrument.map { i in { request in i.attempted(request) } }
            ),
            forURLScheme: BodyView.internalScheme
        )
        instrument?.install(on: configuration)

        web = WKWebView(frame: frame, configuration: configuration)
        web.navigationDelegate = self
        web.uiDelegate = self
        // Q-15: sized by `layout()`, never by autoresizing — its width is a constant.
        web.autoresizingMask = []
        // No back-forward gestures: there is nothing to go back to, and a swipe that appeared
        // to do something would be a navigation this design does not have.
        web.allowsBackForwardNavigationGestures = false
        // Magnification scales the rendered page without laying it out again, so it leaves
        // the viewport — and with it every media query — where Q-15 pinned it.
        web.allowsMagnification = true
        web.setValue(false, forKey: "drawsBackground")

        column.addSubview(web)
        scroller.documentView = column
        scroller.drawsBackground = false
        scroller.hasHorizontalScroller = true
        scroller.hasVerticalScroller = false
        scroller.autohidesScrollers = true
        // The outer view only ever moves sideways; a vertical bounce here would drag the whole
        // column away from the chrome above it.
        scroller.verticalScrollElasticity = .none
        scroller.frame = bounds
        scroller.autoresizingMask = [.width, .height]
        addSubview(scroller)

        // The rule list is added to the live controller, so it binds this web view whether it
        // arrives now or after a first compile. Until it does, `show` holds the document.
        let controller = configuration.userContentController
        BodyViewIsolation.whenReady { [weak self] list in
            guard let self, let list else { return }
            controller.add(list)
            self.isolated = true
            if let html = self.pending {
                self.pending = nil
                self.load(BodyView.document(for: html))
            }
        }
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        needsLayout = true
    }

    /// Lay the pinned column out inside whatever the pane currently is.
    override func layout() {
        super.layout()
        let visible = scroller.contentSize
        let width = max(BodyView.layoutWidth, visible.width)
        column.frame = NSRect(x: 0, y: 0, width: width, height: visible.height)
        // Centred when there is room; flush at the origin, and scrolled to, when there is not.
        // In the narrow case the web view's own vertical scroll bar sits at x = layoutWidth,
        // outside the visible pane until scrolled to: a recorded cost of the pin (dark-mode.md),
        // because no public API exposes the body's scroll position to mirror in a native bar.
        let inset = ((width - BodyView.layoutWidth) / 2).rounded(.down)
        web.frame = NSRect(
            x: inset, y: 0, width: BodyView.layoutWidth, height: visible.height)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not loaded from a nib") }

    static let internalScheme = "sift-resource"

    /// Render a document, revoking the previous one first.
    func show(html: String, token: String, app: OpaquePointer) {
        self.app = app
        // D-90: the previous document's addresses die **before** the next document exists.
        // Doing this after would leave a window in which one token served two messages, which
        // is exactly the property D-28 rests on.
        closeCurrent()
        self.token = token
        present(html)
    }

    /// Put `html` in the body, or hold it until the rule list is installed.
    ///
    /// The part of `show` that has nothing to do with tokens, so the probe can drive it with
    /// no application behind the view.
    func present(_ html: String) {
        guard isolated else {
            // Held rather than loaded without the rule list. If the list never compiles, the
            // view never renders — N-1 is not a property to trade for a visible body.
            pending = html
            return
        }
        load(BodyView.document(for: html))
    }

    /// Load an assembled document at an address minted for this load alone.
    private func load(_ document: String) {
        // Unguessable, so a document cannot name the address its successor will be admitted
        // at; single-use, so it cannot re-enter its own.
        let address = URL(string: "\(BodyView.internalScheme)://document/\(UUID().uuidString)")!
        admitted = address
        // Loaded under the internal scheme so the document's own origin is one nothing else
        // shares — never `about:blank`, which several origins can end up sharing.
        web.loadHTMLString(document, baseURL: address)
    }

    /// The document a body is shown in: the CSP backstop, then a base stylesheet, then `html`.
    ///
    /// `default-src 'none'` with the internal scheme permitted for images and fonts, and
    /// inline styles allowed because the sanitizer's output *is* inline style. The policy is
    /// the first thing in the head, so nothing the body carries is parsed before it applies —
    /// and a second policy the body declares can only narrow it, never widen it.
    static func document(for html: String) -> String {
        """
            <!doctype html><html><head><meta charset="utf-8">
            <meta http-equiv="Content-Security-Policy" content="\
            default-src 'none'; \
            img-src \(BodyView.internalScheme):; \
            font-src \(BodyView.internalScheme):; \
            style-src 'unsafe-inline'; \
            form-action 'none'; \
            base-uri 'none'">
            <style>
              html,body{margin:0;padding:16px;font:-apple-system-body;\
              color:-apple-system-label;background:transparent;\
              word-break:break-word;overflow-wrap:anywhere}
              img{max-width:100%;height:auto}
              table{max-width:100%}
            </style></head><body>\(html)</body></html>
            """
    }

    /// The web view's own data store — NFR-25's per-view store, for the probe to inspect.
    var dataStore: WKWebsiteDataStore { web.configuration.websiteDataStore }

    /// Revoke the current document's token.
    func closeCurrent() {
        guard let app, let token else { return }
        let bytes = Array(token.utf8)
        _ = bytes.withUnsafeBufferPointer { p in
            sift_close_document(UnsafeMutablePointer(app), p.baseAddress, p.count)
        }
        self.token = nil
    }

    /// Show nothing, and stop answering for whatever was there.
    ///
    /// An empty document through the same admission as any other, rather than a bare
    /// `about:blank`: the policy admits nothing it did not mint, and an exception for the
    /// blank page would be an exception a document could aim at.
    func clear() {
        closeCurrent()
        pending = nil
        if isolated { load("") }
    }

    deinit { closeCurrent() }
}

extension BodyView: WKNavigationDelegate, WKUIDelegate {
    func webView(
        _ webView: WKWebView,
        decidePolicyFor navigationAction: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        let url = navigationAction.request.url

        // The load this view started, in the main frame, at the address minted for it —
        // once. Scheme alone is not enough: a refresh to the internal scheme, a subframe
        // under it, or a reload of the current document all share the scheme and are all
        // navigations the document caused rather than the view.
        if let admitted, let url, url == admitted,
            navigationAction.navigationType == .other,
            navigationAction.targetFrame?.isMainFrame == true
        {
            self.admitted = nil
            instrument?.navigated(url, allowed: true)
            decisionHandler(.allow)
            return
        }

        // **Every** other navigation is cancelled. Not filtered, not sanitized — cancelled.
        // A link activation is handed up so the confirmation sheet can show the real
        // destination; anything else is simply refused and nothing is told about it, because
        // a document that could provoke a message is a document with a channel.
        instrument?.navigated(url, allowed: false)
        decisionHandler(.cancel)
        if navigationAction.navigationType == .linkActivated, let url {
            onLink?(url)
        }
    }

    /// A body may not open a window, ever.
    func webView(
        _ webView: WKWebView,
        createWebViewWith configuration: WKWebViewConfiguration,
        for navigationAction: WKNavigationAction,
        windowFeatures: WKWindowFeatures
    ) -> WKWebView? {
        nil
    }
}

/// The internal scheme's handler: the body view's **only** channel out.
///
/// This is a decision function in Sift's own fetch path rather than an interception of
/// somebody else's. A browser extension must intercept requests it does not control; Sift owns
/// every byte, which is the structural claim the whole content-blocking design rests on.
private final class ResourceSchemeHandler: NSObject, WKURLSchemeHandler {
    private let app: (URL) -> OpaquePointer?
    /// Told of **every** attempted load before it is answered — NFR-40's method 2 records
    /// here, and FR-33 item 5 will read the same stream. `nil` in the application today.
    private let observe: ((URLRequest) -> Void)?

    init(app: @escaping (URL) -> OpaquePointer?, observe: ((URLRequest) -> Void)?) {
        self.app = app
        self.observe = observe
    }

    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        observe?(task.request)
        guard let url = task.request.url, let app = app(url) else {
            task.didFailWithError(URLError(.badURL))
            return
        }
        let bytes = Array(url.absoluteString.utf8)
        // Start at unavailable: a resolve that never ran must not read as one that succeeded.
        var answer = SiftResourceAnswer(SiftResourceAnswer_UNAVAILABLE)
        let status = bytes.withUnsafeBufferPointer { p in
            sift_resolve_resource(UnsafeMutablePointer(app), p.baseAddress, p.count, &answer)
        }

        // Blocked and unavailable are both "no bytes", and they are deliberately not the same
        // fact: FR-12 keeps "Sift refused this" and "this did not arrive" distinct, and the
        // count the reader draws comes from the layer rather than from what happened here.
        // Either way the body view is told nothing about which — a document that could tell
        // them apart could probe.
        guard status == Ok, answer == SiftResourceAnswer(SiftResourceAnswer_BYTES) else {
            task.didFailWithError(URLError(.resourceUnavailable))
            return
        }

        // Streaming the bytes themselves is the next piece of this: the broker answers with a
        // length today, and a body with several hundred positions is exactly why they must not
        // all be buffered whole. Until the stream is wired, an allowed resource is reported as
        // unavailable rather than fabricated — an empty image drawn as though it had loaded
        // would be a lie told to the person reading.
        task.didFailWithError(URLError(.resourceUnavailable))
    }

    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {}
}

/// N-1's policy layer for the schemes WebKit supports natively.
///
/// A scheme handler only decides what the engine does not already know how to load. http,
/// https, websockets and blob are built in, so registering nothing for them refuses nothing;
/// and the navigation callback never sees a subresource. A content rule list is the engine's
/// own per-load policy, applied in the content process before a request leaves it, so this is
/// the layer at which those schemes are rejected.
///
/// **Block everything, then except the internal scheme.** `ignore-previous-rules` is how a
/// rule list states an exception, and it overrides only rules before it — so the list denies
/// by default and a scheme nobody thought of is refused rather than admitted.
///
/// Compiled once per process and shared by every body view: the compiled form is immutable
/// and D-54 keeps at most one view per window anyway. It is recompiled at every launch rather
/// than looked up, so a list stored by an older build can never be the one in force.
enum BodyViewIsolation {
    static let identifier = "net.justinchung.sift.body-view.n1"

    static let rules = """
        [{"trigger":{"url-filter":".*"},"action":{"type":"block"}},\
        {"trigger":{"url-filter":"^\(BodyView.internalScheme):"},\
        "action":{"type":"ignore-previous-rules"}}]
        """

    private enum State {
        case idle, compiling, ready(WKContentRuleList), failed
    }
    private static var state = State.idle
    private static var waiting: [(WKContentRuleList?) -> Void] = []

    /// Hand `body` the compiled list — now if it exists, when it does if not, and `nil` if
    /// compiling failed. A `nil` is final: the view that receives it never renders.
    static func whenReady(_ body: @escaping (WKContentRuleList?) -> Void) {
        switch state {
        case .ready(let list):
            body(list)
        case .failed:
            body(nil)
        case .compiling:
            waiting.append(body)
        case .idle:
            waiting.append(body)
            state = .compiling
            WKContentRuleListStore.default().compileContentRuleList(
                forIdentifier: identifier, encodedContentRuleList: rules
            ) { list, error in
                if let list {
                    state = .ready(list)
                } else {
                    state = .failed
                    NSLog("sift: the body view's rule list did not compile, so no body will render: %@",
                          String(describing: error))
                }
                let ready = waiting
                waiting = []
                for each in ready { each(list) }
            }
        }
    }
}

/// NFR-40 method 2's recorder: every load a body view attempts, every navigation it is asked
/// to make, and any script that executes in it.
///
/// This is the instrumentation the P0 spike built and the differential test reuses — "a
/// scheme handler that records **every** attempted load and a bridge that records **any**
/// execution" — and FR-33 items 5 and 9 read the same stream. It is one class so that the
/// three consumers cannot come to disagree about what was observed.
///
/// **The application never constructs one.** A body view holding a recorder registers a
/// message handler that only script could reach, and handlers for schemes the engine would
/// otherwise load itself; neither belongs in a view somebody reads mail in.
///
/// What it records is exactly what reached it. A foreign scheme's loads reach it only once the
/// probe has made the engine willing to hand that scheme to a handler at all (see
/// `BodyViewProbe`), and `data:` and `file:` never do — WebKit resolves both internally. The
/// probe detects those two by what they chain to rather than by this.
final class BodyViewInstrument: NSObject {
    enum Event: Equatable {
        /// A load reached a scheme handler: the internal one, or a foreign one the probe
        /// registered to catch egress the policy layer should have refused.
        case load(URL?)
        /// A navigation was put to the policy callback, and whether it was admitted.
        case navigation(URL?, allowed: Bool)
        /// Script ran and reached the bridge.
        case execution(String)
    }

    /// The message handler name only script can reach.
    static let bridge = "siftProbe"

    /// The schemes whose loads are recorded as egress when the engine can be made to hand
    /// them over. `data` and `file` are listed because the attempt is harmless and the day
    /// WebKit starts consulting a handler for them is a day the probe should see.
    static let foreignSchemes = ["http", "https", "ws", "wss", "blob", "data", "file"]

    /// Bounded, as every buffer here is: a hostile document can attempt loads without end.
    /// Past the bound events are counted rather than kept, and the count is a failure in
    /// itself for anything judging a document by what it did.
    static let capacity = 4096

    private(set) var events: [Event] = []
    private(set) var dropped = 0

    /// The events since the last drain, and how many were dropped past the bound.
    func drain() -> (events: [Event], dropped: Int) {
        defer {
            events = []
            dropped = 0
        }
        return (events, dropped)
    }

    func attempted(_ request: URLRequest) { record(.load(request.url)) }

    func navigated(_ url: URL?, allowed: Bool) { record(.navigation(url, allowed: allowed)) }

    private func record(_ event: Event) {
        if events.count < BodyViewInstrument.capacity {
            events.append(event)
        } else {
            dropped += 1
        }
    }

    /// Register the bridge and every foreign scheme the engine will let a handler take.
    func install(on configuration: WKWebViewConfiguration) {
        configuration.userContentController.add(self, name: BodyViewInstrument.bridge)
        for scheme in BodyViewInstrument.foreignSchemes
        where !WKWebView.handlesURLScheme(scheme) {
            configuration.setURLSchemeHandler(self, forURLScheme: scheme)
        }
    }
}

extension BodyViewInstrument: WKScriptMessageHandler {
    func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage
    ) {
        record(.execution(String(describing: message.body)))
    }
}

extension BodyViewInstrument: WKURLSchemeHandler {
    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
        attempted(task.request)
        task.didFailWithError(URLError(.resourceUnavailable))
    }

    func webView(_ webView: WKWebView, stop task: WKURLSchemeTask) {}
}
