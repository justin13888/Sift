import AppKit

// D-2: **one resident process** holding both the core and the native shell. The web engine's
// content process is created on demand for a message body and destroyed when none is being
// read — which is the whole of how "always running" and "low idle footprint" are both true.

let application = NSApplication.shared
application.delegate = ApplicationShell.shared
// The application is resident with or without a window, so it is an accessory rather than a
// regular application until one opens. Background residency itself is registered through the
// platform's own per-user mechanism — a login item, never a system-wide service and never
// with elevated privileges.
application.setActivationPolicy(.accessory)
application.run()
