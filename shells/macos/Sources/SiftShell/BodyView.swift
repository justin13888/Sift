import AppKit
import CSift
import WebKit

/// D-3's body view: message HTML in its own document, with **no network capability at all**.
///
/// # N-1, in WebKit terms
///
/// The invariant is that only the internal scheme is registered and every other scheme is
/// rejected at the engine's policy layer. Four things enforce it here, and each is load-bearing
/// rather than defence in depth:
///
/// 1. A `WKURLSchemeHandler` is registered for `sift-resource` and nothing else. WebKit will
///    not let a page load a scheme it has no handler for and no built-in support for.
/// 2. `decidePolicyFor` cancels **every** navigation whose scheme is not the internal one, and
///    every navigation at all that is not the initial load. Nothing navigates in place.
/// 3. The data store is **non-persistent**, so two messages share no cookie jar, no cache and
///    no local storage — NFR-25's correlation channel, closed.
/// 4. A `default-src 'none'` content security policy is injected as a backstop. It is a
///    backstop: N-1 is the guarantee, and a CSP that was the guarantee would be one the
///    sanitizer's own parser disagreement could undo.
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

    /// Told when a link is activated, so the confirmation sheet can be raised.
    var onLink: ((URL) -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)

        let configuration = WKWebViewConfiguration()

        // NFR-25: its own store, non-persistent. Two messages cannot correlate through it and
        // neither can observe application state.
        configuration.websiteDataStore = .nonPersistent()

        // D-50, the wide setting. `allowsContentJavaScript` alone would be the narrow one.
        configuration.defaultWebpagePreferences.allowsContentJavaScript = false
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.preferences.isFraudulentWebsiteWarningEnabled = false
        configuration.preferences.setValue(false, forKey: "javaScriptEnabled")

        // N-1's one registered scheme. Everything else has no handler and no built-in support.
        configuration.setURLSchemeHandler(
            ResourceSchemeHandler { [weak self] url in self?.app },
            forURLScheme: BodyView.internalScheme
        )

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

        // The CSP backstop, and a base stylesheet. `default-src 'none'` with the internal
        // scheme permitted for images and fonts, and inline styles allowed because the
        // sanitizer's output *is* inline style.
        let document = """
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
        // Loaded under the internal scheme so the document's own origin is one nothing else
        // shares — never `about:blank`, which several origins can end up sharing.
        web.loadHTMLString(document, baseURL: URL(string: "\(BodyView.internalScheme)://document"))
    }

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
    func clear() {
        closeCurrent()
        web.loadHTMLString("", baseURL: nil)
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
        let scheme = url?.scheme?.lowercased()

        // The initial load of the document Sift assembled. Nothing else is ever allowed.
        if navigationAction.navigationType == .other, scheme == BodyView.internalScheme {
            decisionHandler(.allow)
            return
        }

        // **Every** other navigation is cancelled. Not filtered, not sanitized — cancelled.
        // A link activation is handed up so the confirmation sheet can show the real
        // destination; anything else is simply refused and nothing is told about it, because
        // a document that could provoke a message is a document with a channel.
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

    init(app: @escaping (URL) -> OpaquePointer?) {
        self.app = app
    }

    func webView(_ webView: WKWebView, start task: WKURLSchemeTask) {
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
