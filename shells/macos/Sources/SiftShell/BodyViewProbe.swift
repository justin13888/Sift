import AppKit
import ObjectiveC
import WebKit

/// The P0 body-view spike, runnable: the hostile-HTML corpus against the body view that ships.
///
/// `Sift --probe-body-view <corpus>` loads every sample, raw, into a real [`BodyView`] carrying
/// a [`BodyViewInstrument`], and fails if any sample achieves script execution, network
/// egress, a load under a forbidden scheme, or a navigation in place, or if two body views
/// share a store — the roadmap's pass condition for D-3 and N-1, which is absolute rather than
/// a rate. `mise run body-view-probe` builds and runs it.
///
/// # Controls, because silence is only evidence from a detector that can hear
///
/// Every sample runs twice: in the hardened view, and in a **control** view with script on, no
/// rule list, no CSP and no navigation policy. The control is not judged; it is what proves each
/// detector fires on this engine at all. The run fails if a detector the verdict depends on was
/// never heard from in the control, because a hardened view that "loaded nothing" under a
/// detector that sees nothing has shown nothing.
///
/// # Making egress observable without a network
///
/// There is no listening socket here — NFR-24 forbids one in anything this project builds —
/// so egress is caught before it leaves. WebKit refuses to hand http, https or blob to a
/// registered handler because it supports them itself; for the probe's process alone, and
/// only after this command was given, [`claimForeignSchemes`] makes it willing, so a load that
/// passes the rule list lands in the recorder instead of on the network. The hardened view keeps
/// its rule list, its CSP and its navigation policy in front of that recorder exactly as it
/// does in front of the network, so a load the recorder sees is a load the application would
/// have sent.
///
/// `data:` and `file:` never reach any handler — WebKit resolves both internally — so the
/// corpus detects them by what they chain to: a data or file stylesheet that loaded styles an
/// element with a background at `probe/chain`.
///
/// # What this cannot see
///
/// Connections WebKit opens below the resource layer — `<link rel=preconnect>`,
/// `dns-prefetch`, websockets — reach no handler, and WebKit's networking process opens them
/// where neither `lsof` nor `nettop` run from this process shows them; an ordinary http image
/// sent to the network was not visible either. So the probe has no detector for them, and
/// says so rather than reporting a silence it could not have heard. Websockets need script,
/// which the execution detector covers; the two link hints are closed by the sanitizer, whose
/// allowlist admits no `link` element, and are **not** shown closed by the engine here.
///
/// Storage likewise: a document under the internal scheme writes nothing a store's record list
/// reports even with script on, so a per-sample storage check would be a detector that never
/// fires. NFR-25's property is shown directly instead — two body views' stores are separate
/// objects, and a cookie written into one is found in it and not in the other — and what could
/// write storage from inside a body is script and the network, both covered above.
enum BodyViewProbe {
    /// Run the corpus at `arguments[0]`. The return value is the process's exit status.
    static func run(arguments: [String]) -> Int32 {
        guard let path = arguments.first else {
            FileHandle.standardError.write("usage: Sift --probe-body-view <corpus>\n".data(using: .utf8)!)
            return 2
        }
        let samples: [Sample]
        do {
            samples = try Sample.parse(String(contentsOfFile: path, encoding: .utf8))
        } catch {
            print("probe: cannot read the corpus at \(path): \(error)")
            return 2
        }
        guard !samples.isEmpty else {
            print("probe: the corpus at \(path) has no samples")
            return 2
        }

        NSApplication.shared.setActivationPolicy(.prohibited)
        claimForeignSchemes()

        let fileCSS = FileManager.default.temporaryDirectory
            .appendingPathComponent("sift-probe-\(getpid()).css")
        do {
            try "div.f{background-image:url(\(Marker.chain))}"
                .write(to: fileCSS, atomically: true, encoding: .utf8)
        } catch {
            print("probe: cannot write the file: stylesheet: \(error)")
            return 2
        }
        defer { try? FileManager.default.removeItem(at: fileCSS) }

        return Probe(fileCSS: fileCSS).run(samples)
    }

    /// Make WebKit hand the foreign schemes to a registered handler, **in this process only**.
    ///
    /// `WKWebView.handlesURLScheme` is the public question WebKit asks before refusing to let a
    /// handler take a scheme it supports natively. Answering "no" for the foreign schemes is
    /// what lets the recorder stand where the network would. This is only ever reached from
    /// `--probe-body-view`: in the application the class method is untouched, the instrument is
    /// never constructed, and a foreign load that passed every layer would go to the network —
    /// which is precisely the thing this probe is measuring does not happen.
    static func claimForeignSchemes() {
        guard
            let original = class_getClassMethod(
                WKWebView.self, #selector(WKWebView.handlesURLScheme(_:))),
            let claimed = class_getClassMethod(
                WKWebView.self, #selector(WKWebView.sift_probeHandlesURLScheme(_:)))
        else {
            fatalError("probe: WKWebView.handlesURLScheme is not where it should be")
        }
        method_exchangeImplementations(original, claimed)
    }
}

extension WKWebView {
    /// After [`BodyViewProbe.claimForeignSchemes`], the implementation of `handlesURLScheme`.
    /// Calling itself calls the original, because the two are exchanged.
    @objc class func sift_probeHandlesURLScheme(_ scheme: String) -> Bool {
        if BodyViewInstrument.foreignSchemes.contains(scheme.lowercased()) { return false }
        return sift_probeHandlesURLScheme(scheme)
    }
}

/// The URLs the corpus aims at, and what reaching each one means.
enum Marker {
    static let root = "\(BodyView.internalScheme)://probe/"
    /// The sentinel every sample carries: proof the document rendered.
    static let sentinel = root + "ok"
    /// Reachable only by script.
    static let exec = root + "exec"
    /// Reachable only if a data:, file: or blob: resource loaded.
    static let chain = root + "chain"
    /// Reachable only if a navigation proceeded.
    static let nav = root + "nav"
}

struct Sample {
    let id: String
    let html: String

    static func parse(_ text: String) throws -> [Sample] {
        var samples: [Sample] = []
        var id: String?
        var body: [Substring] = []
        func flush() {
            if let id {
                samples.append(Sample(id: id, html: body.joined(separator: "\n")))
            }
        }
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            if line.hasPrefix("=== ") {
                flush()
                id = String(line.dropFirst(4)).trimmingCharacters(in: .whitespaces)
                body = []
            } else if id != nil {
                body.append(line)
            }
        }
        flush()
        return samples
    }

    func expanded(fileCSS: URL) -> String {
        let exec =
            "(new(Image)).src='\(Marker.exec)';"
            + "window.webkit.messageHandlers.\(BodyViewInstrument.bridge).postMessage('\(id)')"
        return "<img src=\"\(Marker.sentinel)\" width=\"1\" height=\"1\">\n"
            + html
            .replacingOccurrences(of: "{{EXEC}}", with: exec)
            .replacingOccurrences(of: "{{FILE_CSS}}", with: fileCSS.absoluteString)
            .replacingOccurrences(of: "{{ID}}", with: id)
    }
}

/// What one sample did in one view.
struct Observation {
    var rendered = false
    var egress: [String] = []
    var execution: [String] = []
    var chained: [String] = []
    var navigated: [String] = []
    /// blob: loads the engine minted for itself — reported, never counted as egress.
    var engineInternal: [String] = []
    var dropped = 0

    /// Every way this observation breaks N-1 or NFR-20. Empty is a pass.
    var failures: [String] {
        var out: [String] = []
        if !rendered { out.append("the sentinel never loaded, so nothing here is evidence") }
        out += egress.map { "egress: \($0)" }
        out += execution.map { "script executed: \($0)" }
        out += chained.map { "a forbidden scheme loaded: \($0)" }
        out += navigated.map { "navigated in place: \($0)" }
        if dropped > 0 { out.append("\(dropped) events dropped past the recorder's bound") }
        return out
    }

    /// The detectors this observation shows firing — read from the control run.
    var detectors: Set<Detector> {
        var out = Set<Detector>()
        if !egress.isEmpty { out.insert(.egress) }
        if !execution.isEmpty { out.insert(.execution) }
        if !chained.isEmpty { out.insert(.chain) }
        if !navigated.isEmpty { out.insert(.navigation) }
        return out
    }
}

enum Detector: String, CaseIterable {
    case egress, execution, chain, navigation
}

private struct Probe {
    let fileCSS: URL

    /// How long a sample is given to finish what it started after the sentinel loads —
    /// refreshes fire, stylesheets chain.
    let settle: TimeInterval = 1.0
    /// How long a document is given to load the sentinel at all.
    let deadline: TimeInterval = 8.0

    func run(_ samples: [Sample]) -> Int32 {
        let window = NSWindow(
            contentRect: NSRect(x: -20000, y: -20000, width: 800, height: 600),
            styleMask: .borderless, backing: .buffered, defer: false)
        window.orderBack(nil)

        // The hardened view is **one view across the corpus**, as D-90 reuses one across the
        // messages of a reading session: whatever a sample leaves behind is there for the next
        // one to find, which is the correlation NFR-25 closes.
        let instrument = BodyViewInstrument()
        let hardened = BodyView(frame: window.contentLayoutRect, instrument: instrument)
        window.contentView = hardened

        print("probe: \(samples.count) samples, WebKit \(webKitVersion())")
        guard waitForIsolation() else {
            print("probe: FAIL — the rule list never compiled, so the body view never renders")
            return 1
        }

        // NFR-25's separation, shown directly: storage written into one view's store is not
        // visible from another's.
        var failed = !separateStores()

        // Every hardened run back to back, as a reading session would be, then every control.
        var shipped: [Observation] = []
        for sample in samples {
            let html = sample.expanded(fileCSS: fileCSS)
            shipped.append(
                observe(instrument: instrument, html: html, admissions: 1) {
                    hardened.present(html)
                })
        }

        var heard = Set<Detector>()
        var open: [Observation] = []
        for sample in samples {
            let html = sample.expanded(fileCSS: fileCSS)
            let controlInstrument = BodyViewInstrument()
            let control = controlView(instrument: controlInstrument, frame: window.contentLayoutRect)
            window.contentView = control
            open.append(
                observe(instrument: controlInstrument, html: html, admissions: 0) {
                    control.loadHTMLString(
                        "<!doctype html><html><head><meta charset=\"utf-8\"></head><body>\(html)</body></html>",
                        baseURL: URL(string: "\(BodyView.internalScheme)://document/control"))
                })
            heard.formUnion(open[open.count - 1].detectors)
        }
        window.contentView = hardened

        for (index, sample) in samples.enumerated() {
            let problems = shipped[index].failures
            let heardHere = open[index].detectors.map(\.rawValue).sorted().joined(separator: ",")
            let engine = shipped[index].engineInternal.count
            let note = engine == 0 ? "" : "; \(engine) engine-minted blob loads"
            if problems.isEmpty {
                print("  pass  \(sample.id)  (control: \(heardHere.isEmpty ? "inert" : heardHere)\(note))")
            } else {
                failed = true
                print("  FAIL  \(sample.id)")
                for problem in problems { print("          \(problem)") }
            }
        }

        // The verdict rests on every detector having fired in the control at least once.
        for detector in Detector.allCases {
            if heard.contains(detector) {
                print("probe: detector \(detector.rawValue): heard in the control")
            } else {
                print("probe: FAIL — the \(detector.rawValue) detector never fired in the control, so its silence proves nothing")
                failed = true
            }
        }
        print("probe: not observed — preconnect, dns-prefetch and websocket sockets (see BodyViewProbe)")

        print(failed ? "probe: FAIL" : "probe: pass — no sample executed script, egressed, loaded a forbidden scheme, or navigated")
        return failed ? 1 : 0
    }

    /// Load one sample and report everything it did. `admissions` is how many admitted
    /// navigations are the view's own load rather than something the document caused.
    func observe(
        instrument: BodyViewInstrument, html: String, admissions: Int, load: () -> Void
    ) -> Observation {
        _ = instrument.drain()
        load()
        var rendered = false
        spin(for: deadline) {
            rendered = instrument.events.contains(.load(URL(string: Marker.sentinel)))
            return rendered
        }
        spin(for: settle) { false }

        let (events, dropped) = instrument.drain()
        var o = Observation(rendered: rendered, dropped: dropped)
        var admitted = 0
        for event in events {
            switch event {
            case .load(let url):
                let text = url?.absoluteString ?? "<no url>"
                let scheme = url?.scheme?.lowercased()
                // A blob: address the document never contained was minted by the engine: a
                // page cannot mint one without script, and the execution detector is what
                // establishes that none ran. A bare `<video>` — no source, no script — makes
                // WebKit load thirteen of them for its own use, past the rule list. They resolve
                // in the engine's in-process blob registry, and the recorder only sees them
                // because the probe claimed the scheme, so they are reported and are not
                // egress. The sanitizer admits no media element in any case.
                if scheme == "blob", !html.contains(text) {
                    o.engineInternal.append(text)
                    continue
                }
                guard let url, scheme == BodyView.internalScheme else {
                    o.egress.append(text)
                    continue
                }
                switch text {
                case Marker.exec: o.execution.append("loaded \(text)")
                case Marker.chain: o.chained.append("loaded \(text)")
                case Marker.nav: o.navigated.append("loaded \(text)")
                default: break  // an internal load: the broker's to decide, and N-1 permits it
                }
            case .navigation(let url, let allowed):
                guard allowed else { continue }
                admitted += 1
                // The view's own load is admitted; anything past it the document caused.
                if admitted > admissions { o.navigated.append(url?.absoluteString ?? "<no url>") }
            case .execution(let body):
                o.execution.append("bridge received '\(body)'")
            }
        }
        return o
    }

    /// Wait for the rule list the hardened view will not render without.
    func waitForIsolation() -> Bool {
        var outcome: Bool?
        BodyViewIsolation.whenReady { outcome = $0 != nil }
        spin(for: 30) { outcome != nil }
        return outcome == true
    }

    /// Write a cookie into one body view's store and look for it from another's.
    func separateStores() -> Bool {
        let a = BodyView(frame: .zero, instrument: nil)
        let b = BodyView(frame: .zero, instrument: nil)
        guard a.dataStore !== b.dataStore else {
            print("probe: FAIL — two body views share one data store")
            return false
        }
        guard
            let cookie = HTTPCookie(properties: [
                .domain: "document", .path: "/", .name: "correlate", .value: "1",
            ])
        else { return false }
        var set = false
        a.dataStore.httpCookieStore.setCookie(cookie) { set = true }
        spin(for: 5) { set }
        var seen: [HTTPCookie]?
        b.dataStore.httpCookieStore.getAllCookies { seen = $0 }
        spin(for: 5) { seen != nil }
        var own: [HTTPCookie]?
        a.dataStore.httpCookieStore.getAllCookies { own = $0 }
        spin(for: 5) { own != nil }
        guard own?.contains(where: { $0.name == "correlate" }) == true else {
            print("probe: FAIL — a cookie written to a view's own store was not there to find, so the check below proves nothing")
            return false
        }
        guard seen?.isEmpty == true else {
            print("probe: FAIL — a cookie written to one body view's store is visible from another's")
            return false
        }
        print("probe: pass  two body views hold separate, non-persistent stores")
        return true
    }

    /// A view with none of the body view's defences: script on, no rule list, no CSP, no
    /// navigation policy. What it records is what each vector does when nothing stops it.
    func controlView(instrument: BodyViewInstrument, frame: NSRect) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.setURLSchemeHandler(instrument, forURLScheme: BodyView.internalScheme)
        instrument.install(on: configuration)
        let view = ControlView(frame: frame, configuration: configuration)
        view.instrument = instrument
        view.navigationDelegate = view
        return view
    }

    func webKitVersion() -> String {
        Bundle(for: WKWebView.self).object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "?"
    }
}

/// The control's navigation delegate admits everything and records it, so a navigation the
/// hardened view refused is shown to be one the vector really makes.
private final class ControlView: WKWebView, WKNavigationDelegate {
    var instrument: BodyViewInstrument?
    private var first = true

    func webView(
        _ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        // The control's own load is not a navigation the document caused.
        if first, action.targetFrame?.isMainFrame == true {
            first = false
        } else {
            instrument?.navigated(action.request.url, allowed: true)
        }
        decisionHandler(.allow)
    }
}

/// Run the main loop until `done` holds or `seconds` pass.
private func spin(for seconds: TimeInterval, until done: () -> Bool) {
    let end = Date().addingTimeInterval(seconds)
    while Date() < end, !done() {
        RunLoop.main.run(mode: .default, before: Date().addingTimeInterval(0.02))
    }
}
