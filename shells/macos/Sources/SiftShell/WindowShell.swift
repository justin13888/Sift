import AppKit
import CSift

/// A window, and everything that dies with it.
///
/// **A window is a view onto the presentation layer, never an instance of it.** Windows are
/// plural, and anything scoped to "a window" that is really "any window open" has to say the
/// latter — the filter engine's lifetime is the case that matters, since it is bound to
/// window lifetime and dropped at L1.
final class WindowShell: NSObject, NSWindowDelegate {
    private let app: OpaquePointer?
    private var window: NSWindow?
    private var generation: Generation = Generation(0)

    init(app: OpaquePointer?) {
        self.app = app
    }

    func show() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1100, height: 700),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.delegate = self
        window.title = "Sift"
        window.contentView = NSView()
        window.makeKeyAndOrderFront(nil)
        self.window = window

        observeMessages()
    }

    /// D-18 — an observation over a declared window of a result set.
    ///
    /// **Anchored, not an integer range.** A shell holding a range would recompute it on
    /// every notification, which is the polling D-18 rejected wearing different clothes.
    private func observeMessages() {
        guard let app else { return }
        var generation = Generation(0)
        _ = sift_observe_messages(
            UnsafeMutablePointer(app),
            // The anchor. D-18 anchors an observation on an identity rather than an integer
            // range, so that the layer maintains the window across change instead of the
            // shell recomputing it on every notification.
            SiftId(bytes: (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)),
            200,
            { _, generation, rows in
                // Receive, record, return. The boundary is **not reentrant**: calling back
                // into the layer from here is the thing D-48 forbids, and acting on the next
                // turn of the loop is the shell author's rule.
                _ = generation
                _ = rows
            },
            nil,
            &generation
        )
        self.generation = generation
    }

    func windowWillClose(_ notification: Notification) {
        // Cancellation is **synchronous**: when this returns, no further callback for that
        // observation arrives on any thread, ever. It can block briefly, so it must never be
        // called from somewhere that cannot afford to wait — and a stale delivery already
        // posted to this loop is discarded by comparing generations rather than waited for,
        // because waiting would be waiting on this loop from this loop.
        if let app { _ = sift_cancel_observation(UnsafeMutablePointer(app), generation) }
    }
}
