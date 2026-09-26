import AppKit

// D-2: **one resident process** holding both the core and the native shell. The web engine's
// content process is created on demand for a message body and destroyed when none is being
// read — which is the whole of how "always running" and "low idle footprint" are both true.

let application = NSApplication.shared

// The P0 body-view spike and NFR-40 method 2: the hostile-HTML corpus against the body view
// that ships, then exit. Checked before the application shell exists, so the probe runs with
// no core, no container and no window of Sift's own — only body views it made itself.
if let flag = CommandLine.arguments.firstIndex(of: "--probe-body-view") {
    exit(BodyViewProbe.run(arguments: Array(CommandLine.arguments[(flag + 1)...])))
}

application.delegate = ApplicationShell.shared
// The application is resident with or without a window, so it is an accessory rather than a
// regular application until one opens. `LSUIElement` in the bundle states the same thing to
// the launch machinery, so Sift never bounces into the dock on its way to the menu bar; this
// call is what holds it before the delegate has run. `ApplicationShell` raises the policy to
// `.regular` while any window is open and lowers it again when the last one closes, which is
// what makes "until one opens" true rather than aspirational.
//
// Note what that means today: a launch opens a window either way — the mail if the container
// holds an account, the add-account screen if it does not. The window-less accessory state is
// reached by closing it, not by starting.
//
// Background residency itself is registered through the platform's own per-user mechanism — a
// login item, never a system-wide service and never with elevated privileges.
application.setActivationPolicy(.accessory)
application.run()
