import AppKit

// D-2: **one resident process** holding both the core and the native shell. The web engine's
// content process is created on demand for a message body and destroyed when none is being
// read — which is the whole of how "always running" and "low idle footprint" are both true.

let application = NSApplication.shared
application.delegate = ApplicationShell.shared
// The application is resident with or without a window, so it is an accessory rather than a
// regular application until one opens. `LSUIElement` in the bundle states the same thing to
// the launch machinery, so Sift never bounces into the dock on its way to the menu bar; this
// call is what holds it before the delegate has run. `ApplicationShell` raises the policy to
// `.regular` while any window is open and lowers it again when the last one closes, which is
// what makes "until one opens" true rather than aspirational.
//
// Note what that means today: `hasAnyAccount()` is a stub answering false, so every launch
// takes the add-account branch and opens a window immediately. The window-less accessory state
// is reached by closing it, not by starting.
//
// Background residency itself is registered through the platform's own per-user mechanism — a
// login item, never a system-wide service and never with elevated privileges.
application.setActivationPolicy(.accessory)
application.run()
