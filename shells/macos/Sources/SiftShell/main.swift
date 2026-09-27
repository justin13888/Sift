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

// The P0 accessibility spike (NFR-50, R-14): a body view, and a second process reading its tree
// as a screen reader would. The client half is this same binary, launched by the host half.
if let flag = CommandLine.arguments.firstIndex(of: AccessibilityProbe.clientFlag) {
    exit(AccessibilityProbe.client(arguments: Array(CommandLine.arguments[(flag + 1)...])))
}
if CommandLine.arguments.contains(AccessibilityProbe.flag) {
    exit(AccessibilityProbe.run())
}

// The P0 residency spike's registration half: can this sandboxed bundle register itself as a
// login item? It restores whatever state it found, then exits.
if CommandLine.arguments.contains("--probe-login-item") {
    exit(LoginItem.probe())
}

// D-35: core dumps off and the crash handler in place before the application shell exists, so
// nothing the layer reads into memory — a credential included — is ever in a dump. After the
// probes, which run with no container and so have nowhere to put a report.
Diagnostics.install()
// D-114: last run's report, shown once the run loop is up. Queued rather than run here, so it
// is presented as a modal over a running application rather than before there is one.
if Diagnostics.pendingReport != nil {
    DispatchQueue.main.async { Diagnostics.presentPendingReport() }
}

application.delegate = ApplicationShell.shared
// The application is resident with or without a window, so it is an accessory rather than a
// regular application until one opens. `LSUIElement` in the bundle states the same thing to
// the launch machinery, so Sift never bounces into the dock on its way to the menu bar; this
// call is what holds it before the delegate has run. `ApplicationShell` raises the policy to
// `.regular` while any window is open and lowers it again when the last one closes, which is
// what makes "until one opens" true rather than aspirational.
//
// Note what that means: a launch the user made opens a window — the mail if the container
// holds an account, the add-account screen if it does not. A launch the login item made opens
// none when there is an account, so the window-less accessory state is where it starts.
//
// Background residency itself is registered through the platform's own per-user mechanism — a
// login item registering this application (`LoginItem`, the tray's "Open at Login"), never a
// system-wide service, never a helper executable, and never with elevated privileges.
application.setActivationPolicy(.accessory)
application.run()
