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
    /// The observation's **identity**, which is what cancellation takes.
    ///
    /// Not a generation. A generation is a per-delivery staleness stamp; cancelling by one
    /// would cancel every observation sharing it.
    private var observation: SiftObservation = SiftObservation(SiftObservation_NONE)

    /// How this shell announces that it is finished.
    ///
    /// A window shell that closes silently is one the application shell still holds, and the
    /// application shell's window list is what decides whether Sift is in the dock. Closing a
    /// window has to be observable for that to stay true.
    private let onClose: (WindowShell) -> Void

    init(app: OpaquePointer?, onClose: @escaping (WindowShell) -> Void) {
        self.app = app
        self.onClose = onClose
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

        // **This shell owns the window's lifetime; AppKit must not also own it.**
        // `isReleasedWhenClosed` defaults to true for a window built this way, which under ARC
        // means AppKit releases it on close *and* this strong reference releases it again. The
        // second release lands on freed memory, and it lands late — inside the deferred dealloc
        // of the close animation, on a later turn of the run loop — so it reads as the
        // application vanishing rather than as anything to do with closing a window.
        window.isReleasedWhenClosed = false
        window.contentView = NSView()
        // Windows are plural, so a second one must not land exactly on the first.
        window.center()
        window.makeKeyAndOrderFront(nil)
        self.window = window

        observeMessages()
    }

    /// Bring this window forward without building another one.
    func raise() {
        window?.makeKeyAndOrderFront(nil)
    }

    /// D-18 — an observation over a declared window of a result set.
    ///
    /// **Anchored, not an integer range.** A shell holding a range would recompute it on
    /// every notification, which is the polling D-18 rejected wearing different clothes.
    private func observeMessages() {
        guard let app else { return }
        var observation = SiftObservation(SiftObservation_NONE)
        _ = sift_observe_messages(
            UnsafeMutablePointer(app),
            // The anchor. D-18 anchors an observation on an identity rather than an integer
            // range, so that the layer maintains the window across change instead of the
            // shell recomputing it on every notification.
            SiftId(bytes: (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)),
            200,
            { _, observation, generation, rows in
                // Receive, record, return. The boundary is **not reentrant**: calling back
                // into the layer from here is the thing D-48 forbids, and acting on the next
                // turn of the loop is the shell author's rule.
                //
                // Both identifiers arrive because they answer different questions: the
                // observation says which registration this is for, the generation says
                // whether it is still wanted.
                _ = observation
                _ = generation
                _ = rows
            },
            nil,
            &observation
        )
        self.observation = observation
    }

    func windowWillClose(_ notification: Notification) {
        // Cancellation is **synchronous**: when this returns, no further callback for that
        // observation arrives on any thread, ever. It can block briefly, so it must never be
        // called from somewhere that cannot afford to wait — and a stale delivery already
        // posted to this loop is discarded by comparing generations rather than waited for,
        // because waiting would be waiting on this loop from this loop.
        if let app { _ = sift_cancel_observation(UnsafeMutablePointer(app), observation) }

        // Cancellation first, and it stays synchronous — that is D-48's rule and the whole
        // guarantee is that when it returns, nothing further arrives for this observation.
        //
        // Telling the application shell is the opposite: it is what drops the last reference to
        // this shell and therefore to the window, and doing that here would free the window
        // while AppKit is still closing it. So it waits for the next turn of the loop, which is
        // this shell's rule everywhere else too — receive, record, return.
        DispatchQueue.main.async { [self] in onClose(self) }
    }
}
