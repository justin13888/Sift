import AppKit
import ApplicationServices
import WebKit

/// The P0 accessibility spike, runnable: NFR-50's tree, read across the body view's process and
/// sandbox by a separate assistive client, the way a screen reader reads it.
///
/// `Sift --probe-accessibility` shows a fixture document in a real [`BodyView`] — the hardened
/// configuration that ships: its own document, WebKit's separate and sandboxed content process,
/// a non-persistent store minted for the view, script disabled engine-wide, and the rule list
/// and CSP in force — and then launches **a second process**, this same binary in
/// `--probe-accessibility-client` mode, which walks the host's accessibility hierarchy through
/// the platform's assistive-technology interface and nothing else. That interface is the one a
/// screen reader consumes, so what the client finds is what a screen reader has to announce.
/// `mise run accessibility-probe` builds and runs it.
///
/// # What passes
///
/// The client must find, **under the host's window** — part of the reader rather than a
/// detached document somewhere else — a web area whose content is served by a process other than
/// the host's (the tree really crossed the process boundary rather than being mirrored), and in it:
/// the body text, a heading at its level, an image by its alternative text, a table with its rows
/// and columns, paragraphs in reading order, and a link as a link. It must **not** find text the
/// document does not contain, so a client that reported everything would fail.
///
/// # N-1, for the bridge
///
/// Sift writes no bridge: the tree crosses through the engine's own remote accessibility, so the
/// bridge exposes structure and text outward and adds no channel inward. What the probe checks is
/// the one inward thing an assistive client can do — act — and that reading causes nothing:
/// while the client walks the tree the body view must attempt no load, admit no navigation, and
/// run no script; and a link the client *presses* must reach the same navigation policy as a
/// click and be refused in place, handed up for confirmation like any other link.
///
/// # What this cannot see
///
/// It reads the tree a screen reader reads; it does not listen to a screen reader speak. A
/// VoiceOver pass over the same reader is the human half of the spike, and the pull request that
/// introduced this names it. It needs the assistive-access grant the platform requires of any
/// client (the terminal that launches it holds it); without one it exits 3 as *unavailable*
/// rather than reporting a failure it could not have observed.
enum AccessibilityProbe {
    static let flag = "--probe-accessibility"
    static let clientFlag = "--probe-accessibility-client"

    /// The document the client must find. Every assertion below names something in it.
    static let fixture = """
        <img src="\(Marker.sentinel)" width="1" height="1" alt="">
        <h2>Quarterly figures</h2>
        <p>First paragraph, carrying the sift-needle.</p>
        <p>Second paragraph.</p>
        <img src="\(BodyView.internalScheme)://probe/chart" width="40" height="20" alt="A red bar chart">
        <table>
          <tr><th>Quarter</th><th>Revenue</th></tr>
          <tr><td>First</td><td>Nine</td></tr>
        </table>
        <p>Third paragraph, with <a href="https://example.invalid/linked">a link out</a>.</p>
        """

    static let linkTarget = "https://example.invalid/linked"

    /// Host mode. The return value is the process's exit status.
    static func run() -> Int32 {
        guard AXIsProcessTrusted() else {
            print("""
                accessibility-probe: UNAVAILABLE — this process has no assistive-access grant, so no \
                client it launches can read any tree. Grant Accessibility to the terminal in System \
                Settings > Privacy & Security > Accessibility and run it again.
                """)
            return 3
        }
        let application = NSApplication.shared
        // An accessory has no dock icon, and unlike `.prohibited` it still answers assistive
        // clients once launching has finished — which a bare `run` loop never does by itself.
        application.setActivationPolicy(.accessory)
        application.finishLaunching()

        let window = NSWindow(
            contentRect: NSRect(x: -20000, y: -20000, width: 800, height: 600),
            styleMask: .borderless, backing: .buffered, defer: false)
        window.orderBack(nil)
        let instrument = BodyViewInstrument()
        let view = BodyView(frame: window.contentLayoutRect, instrument: instrument)
        var handedUp: [URL] = []
        view.onLink = { handedUp.append($0) }
        window.contentView = view

        view.present(fixture)
        var rendered = false
        spin(for: 30) {
            rendered = instrument.events.contains(.load(URL(string: Marker.sentinel)))
            return rendered
        }
        guard rendered else {
            print("accessibility-probe: FAIL — the fixture never rendered, so nothing here is evidence")
            return 1
        }
        spin(for: 1) { false }
        _ = instrument.drain()

        guard let executable = Bundle.main.executableURL else {
            print("accessibility-probe: FAIL — no executable to launch the client from")
            return 1
        }
        let client = Process()
        client.executableURL = executable
        client.arguments = [clientFlag, String(getpid())]
        do {
            try client.run()
        } catch {
            print("accessibility-probe: FAIL — the client did not launch: \(error)")
            return 1
        }
        // The host answers the client from its main run loop, so it must keep spinning.
        spin(for: 90) { !client.isRunning }
        if client.isRunning {
            client.terminate()
            print("accessibility-probe: FAIL — the client did not finish within 90 s")
            return 1
        }
        spin(for: 1) { false }

        var failed = client.terminationStatus != 0

        // N-1: everything the body view did while an assistive client read and acted on it.
        let (events, dropped) = instrument.drain()
        var caused: [String] = []
        var refusedLink = false
        for event in events {
            switch event {
            case .navigation(let url, allowed: false) where url?.absoluteString == linkTarget:
                refusedLink = true
            case .navigation(let url, let allowed):
                caused.append("\(allowed ? "admitted" : "refused") a navigation to \(url?.absoluteString ?? "<no url>")")
            case .load(let url):
                caused.append("attempted a load of \(url?.absoluteString ?? "<no url>")")
            case .execution(let body):
                caused.append("ran script that reached the bridge with '\(body)'")
            }
        }
        if dropped > 0 { caused.append("\(dropped) events dropped past the recorder's bound") }
        if caused.isEmpty {
            print("accessibility-probe: pass  reading the tree loaded nothing, admitted nothing and ran nothing")
        } else {
            failed = true
            print("accessibility-probe: FAIL — while the client read and acted, the body view:")
            for each in caused { print("          \(each)") }
        }
        if refusedLink, handedUp.map(\.absoluteString) == [linkTarget] {
            print("accessibility-probe: pass  a link pressed by the client was refused in place and handed up for confirmation")
        } else {
            failed = true
            print("accessibility-probe: FAIL — a link pressed by the client was \(refusedLink ? "refused" : "never put to the navigation policy") and handed up \(handedUp.count) times")
        }

        print(failed ? "accessibility-probe: FAIL" : "accessibility-probe: pass — body content crosses to an assistive client as part of the reader, and the crossing is outward only")
        return failed ? 1 : 0
    }

    /// Client mode: a separate process reading `pid`'s tree as a screen reader would.
    static func client(arguments: [String]) -> Int32 {
        guard let text = arguments.first, let pid = pid_t(text) else {
            print("accessibility-probe client: usage: Sift \(clientFlag) <pid>")
            return 2
        }
        let application = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(application, 5)

        // The content process builds its tree on first request, so the first walk may find the
        // web area empty. Walk until it is not, within a bound.
        var tree: Tree?
        let deadline = Date().addingTimeInterval(30)
        repeat {
            tree = Tree(application: application, host: pid)
            if let tree, !tree.content.isEmpty { break }
            Thread.sleep(forTimeInterval: 0.5)
        } while Date() < deadline

        guard let tree, tree.webArea != nil else {
            print("accessibility-probe: FAIL — no web area under the host's window; the body is not in the reader's tree at all")
            return 1
        }
        var ok = true
        func check(_ passed: Bool, _ pass: String, _ fail: @autoclosure () -> String) {
            if passed {
                print("accessibility-probe: pass  \(pass)")
            } else {
                ok = false
                print("accessibility-probe: FAIL — \(fail())")
            }
        }

        check(
            tree.webAreaInWindow,
            "the body's web area sits under the host's window, as part of the reader",
            "the web area is not a descendant of the host's window")
        let crossed = Set(tree.content.map(\.pid)).subtracting([pid, 0])
        check(
            !crossed.isEmpty,
            "the body's content is served by \(crossed.sorted().map(processName).joined(separator: ", ")), not the host (\(pid)): the tree crossed",
            "every element under the web area belongs to the host, so nothing crossed a process boundary")

        let texts = tree.content.filter { $0.role == kAXStaticTextRole }.map(\.value)
        check(
            texts.contains { $0.contains("sift-needle") },
            "the body text is exposed",
            "no text element carries the body text; text found: \(texts)")
        check(
            !texts.contains { $0.contains("sift-absent-word") } && !tree.content.contains { $0.title.contains("sift-absent-word") },
            "text the document does not contain is not found",
            "the client found text the document does not contain")

        let headings = tree.content.filter { $0.role == "AXHeading" }
        check(
            headings.contains { $0.value == "2" && $0.label.contains("Quarterly figures") },
            "the heading is exposed at level 2",
            "no level-2 heading named 'Quarterly figures'; headings: \(headings.map { "\($0.value) \($0.label)" })")

        let images = tree.content.filter { $0.role == kAXImageRole }
        check(
            images.contains { $0.label == "A red bar chart" },
            "the image is announced by its alternative text",
            "no image described as 'A red bar chart'; images: \(images.map(\.label))")

        let rows = tree.content.filter { $0.role == kAXRowRole }
        let columns = tree.content.filter { $0.role == kAXColumnRole }
        let hasTable = tree.content.contains { $0.role == kAXTableRole }
        let cells = ["Quarter", "Revenue", "First", "Nine"]
        let cellsInOrder = texts.filter(cells.contains).prefix(4)
        check(
            hasTable && rows.count == 2 && columns.count == 2 && Array(cellsInOrder) == cells,
            "the table is exposed with its 2 rows, 2 columns and cells in order",
            "table \(hasTable), \(rows.count) rows, \(columns.count) columns, cells \(Array(cellsInOrder))")

        let order = ["First paragraph", "Second paragraph", "Third paragraph"].map { needle in
            texts.firstIndex { $0.hasPrefix(needle) }
        }
        check(
            order.allSatisfy { $0 != nil } && order.compactMap { $0 } == order.compactMap({ $0 }).sorted(),
            "the paragraphs are read in document order",
            "reading order is \(order)")

        let link = tree.content.first { $0.role == "AXLink" }
        check(
            link != nil,
            "the link is exposed as a link",
            "no link element in the body")
        if let link {
            // The inward act an assistive client has. The host judges what it caused.
            let pressed = AXUIElementPerformAction(link.element, kAXPressAction as CFString)
            check(
                pressed == .success,
                "the client pressed the link",
                "pressing the link failed with \(pressed.rawValue)")
        }
        return ok ? 0 : 1
    }
}

/// One element of the tree, as the client read it.
private struct Node {
    let element: AXUIElement
    let pid: pid_t
    let role: String
    let value: String
    let title: String
    let description: String

    /// What a screen reader would announce it as: its title, else its description. A heading's
    /// title is filled from its text children while walking.
    var label: String { !title.isEmpty ? title : description }
}

/// The host's tree, walked from the application element down.
private struct Tree {
    /// How many elements the walk has visited, to bound it.
    private var visited = 0
    /// The first web area found under a window.
    private(set) var webArea: Node?
    private(set) var webAreaInWindow = false
    /// Everything under the web area, in the order a screen reader traverses it.
    private(set) var content: [Node] = []

    init(application: AXUIElement, host: pid_t) {
        walk(application, depth: 0, inWindow: false, inWeb: false)
    }

    private mutating func walk(_ element: AXUIElement, depth: Int, inWindow: Bool, inWeb: Bool) {
        guard depth < 64, visited < 4096 else { return }
        visited += 1
        let pid = servingProcess(element)
        let role = string(element, kAXRoleAttribute)
        var node = Node(
            element: element, pid: pid, role: role,
            value: string(element, kAXValueAttribute),
            title: string(element, kAXTitleAttribute),
            description: string(element, kAXDescriptionAttribute))
        // A heading's name is its text child; carry it up so the heading reads as announced.
        if role == "AXHeading", node.title.isEmpty {
            let text = children(element).map { string($0, kAXValueAttribute) }.joined()
            node = Node(
                element: element, pid: pid, role: role, value: node.value, title: text,
                description: node.description)
        }
        let window = inWindow || role == kAXWindowRole
        var web = inWeb
        if role == "AXWebArea", webArea == nil {
            webArea = node
            webAreaInWindow = window
            web = true
        } else if inWeb {
            content.append(node)
        }
        for child in children(element) {
            walk(child, depth: depth + 1, inWindow: window, inWeb: web)
        }
    }
}

/// The process that answers for `element`.
///
/// `AXUIElementGetPid` reports the application an element belongs to, which for the body's
/// content is the host — the remote tree is grafted into the host's so that a screen reader sees
/// one application. The element's own description names the process it is actually served by,
/// and that is the fact this probe needs: that the tree crossed rather than being mirrored.
/// `0` when the description does not say.
private func servingProcess(_ element: AXUIElement) -> pid_t {
    let text = CFCopyDescription(element) as String
    guard let range = text.range(of: "pid=") else { return 0 }
    return pid_t(text[range.upperBound...].prefix { $0.isNumber }) ?? 0
}

/// `pid` and the name of its executable, so the report says *which* process served the tree.
private func processName(_ pid: pid_t) -> String {
    var buffer = [CChar](repeating: 0, count: 1024)
    let length = proc_name(pid, &buffer, UInt32(buffer.count))
    let name = length > 0 ? String(cString: buffer) : "an unnamed process"
    return "\(name) (\(pid))"
}

private func children(_ element: AXUIElement) -> [AXUIElement] {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &value) == .success
    else { return [] }
    return value as? [AXUIElement] ?? []
}

private func string(_ element: AXUIElement, _ attribute: String) -> String {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success,
        let value
    else { return "" }
    if let text = value as? String { return text }
    if let number = value as? NSNumber { return number.stringValue }
    return ""
}
