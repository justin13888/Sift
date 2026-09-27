import AppKit
import Darwin
import MachO

/// D-35, D-114 and Q-20 — the crash report, and what a QA pass collects without a terminal.
///
/// **What a report holds, and what it never can.** D-35 allows backtraces, per-subsystem
/// counters and build metadata, and forbids heap memory. The report written here is a
/// backtrace of the thread that crashed, the signal and faulting address, the version, build
/// and source revision, and the UUID and load address of every image loaded at launch — which
/// is what offline symbolication against the kept dSYM needs. It is assembled from the stack
/// and from text prepared at launch, and nothing in the handler reads the heap. That is a
/// property of *how it is written*, not a scrub applied afterwards, which is the distinction
/// D-35 is built on.
///
/// **Where it goes.** One file in the container's `Diagnostics` directory, beside NFR-55's log.
/// D-114: only the most recent report is kept, a new crash replaces an unsent one, and Sift
/// makes no connection for it. At the next launch it is shown in full, and the user can save a
/// copy, reveal it in Finder, or dismiss it — which deletes it.
///
/// **Core dumps.** D-35 requires the operating system's own dumps be disabled for the process.
/// `RLIMIT_CORE` is lowered to zero, hard limit included, before anything else is set up, and
/// the outcome is read back rather than assumed: it is shown in the runtime panel and stamped
/// into every report, so the answer Q-20 asks for is recorded against whichever build is
/// running — the sandboxed Developer ID build included.
///
/// **What cannot be disabled.** The platform's own crash reporter runs outside the process and
/// no application can turn it off; Q-20's other half. Besides backtraces and the loaded
/// libraries it keeps each thread's register state and application-specific text copied out
/// of the process, so nothing establishes that it is free of heap-derived data; it shares
/// anything only under the user's own analytics settings. The runtime panel states it as a
/// limitation rather than implying D-35's guarantee covers it.
enum Diagnostics {
    /// `sift_observe::log::DIRECTORY`, `CRASH_REPORT` and `BUDGET_BYTES` (L-33). The Rust log
    /// and this shell name the same files; the spellings change together.
    static let directoryName = "Diagnostics"
    static let reportName = "crash-report.txt"
    static let logNames = ["diagnostic.log", "diagnostic.log.1"]
    static let logBudgetBytes: Int64 = 4 * 1024 * 1024
    /// L-34, in days, for the sentence the runtime panel shows.
    static let logRetentionDays = 7

    /// What happened when core dumps were disabled at launch.
    enum CoreDumps {
        case notAttempted
        case disabled
        /// `setrlimit` refused, or the limit read back was not zero.
        case stillEnabled(errno: Int32)
    }

    private(set) static var coreDumps = CoreDumps.notAttempted

    /// The report the previous run left, read before this run could replace it.
    private(set) static var pendingReport: String?

    /// `Application Support/net.justinchung.sift/Diagnostics` — under the sandbox, inside the
    /// app's own container. The same root `ApplicationShell` hands the layer.
    static var directory: URL? {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("net.justinchung.sift", isDirectory: true)
            .appendingPathComponent(directoryName, isDirectory: true)
    }

    static var reportURL: URL? { directory?.appendingPathComponent(reportName) }

    /// Called once, from `main`, before the application shell exists.
    static func install() {
        coreDumps = disableCoreDumps()
        guard let directory else { return }
        try? FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        if let url = reportURL, let text = try? String(contentsOf: url, encoding: .utf8) {
            pendingReport = text
        }
        guard let path = reportURL?.path else { return }
        prepareHandler(path: path, header: header())
    }

    // MARK: - Core dumps

    private static func disableCoreDumps() -> CoreDumps {
        var none = rlimit(rlim_cur: 0, rlim_max: 0)
        let failed = setrlimit(RLIMIT_CORE, &none) != 0 ? errno : 0
        var now = rlimit()
        guard getrlimit(RLIMIT_CORE, &now) == 0 else { return .stillEnabled(errno: errno) }
        return now.rlim_cur == 0 && now.rlim_max == 0 ? .disabled : .stillEnabled(errno: failed)
    }

    static var coreDumpSentence: String {
        switch coreDumps {
        case .disabled:
            return "Core dumps are off for this process."
        case .stillEnabled(let code):
            return "Core dumps could not be turned off (error \(code)). A crash may leave a full "
                + "memory dump if the system is set to write them."
        case .notAttempted:
            return "Core dumps were not turned off."
        }
    }

    /// Q-20's limitation, stated in the interface as that question requires.
    static let platformReporterSentence =
        "macOS's own crash reporter also records a crash, and no app can turn it off. It keeps "
        + "backtraces, the list of loaded libraries, register values and some text taken from "
        + "Sift at the moment of the crash, so Sift cannot promise it holds none of your data. "
        + "It shares them only as your Analytics & Improvements settings allow."

    // MARK: - The report

    /// Everything the report says that can be known before a crash, prepared now so the
    /// handler has only to copy it.
    private static func header() -> String {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? "unknown"
        let build = info["CFBundleVersion"] as? String ?? "unknown"
        var revision = info["SiftSourceRevision"] as? String ?? ""
        if revision.isEmpty || revision.hasPrefix("$(") { revision = "not recorded" }
        #if arch(arm64)
        let arch = "arm64"
        #else
        let arch = "x86_64"
        #endif
        let started = ISO8601DateFormatter().string(from: Date())
        let dumps: String
        switch coreDumps {
        case .disabled: dumps = "disabled"
        case .stillEnabled(let code): dumps = "could not be disabled (errno \(code))"
        case .notAttempted: dumps = "not attempted"
        }
        var lines = [
            "Sift crash report",
            "",
            "This file holds a backtrace of the thread that crashed and the identifiers needed to",
            "read it. It holds nothing from Sift's memory: no message, address, subject or password.",
            "Sift never sends it anywhere.",
            "",
            "version: \(version) (build \(build))",
            "revision: \(revision)",
            "architecture: \(arch)",
            "macOS: \(ProcessInfo.processInfo.operatingSystemVersionString)",
            "started: \(started)",
            "core dumps: \(dumps)",
            "",
            "binary images at launch (uuid, load address, path):",
        ]
        lines.append(contentsOf: images())
        lines.append("")
        return lines.joined(separator: "\n") + "\n"
    }

    /// Every loaded image's UUID and load address, which is what `atos` needs to symbolicate a
    /// frame against the dSYM a release keeps. Paths under the home directory are shortened to
    /// `~`, so the report does not carry the account name.
    private static func images() -> [String] {
        // The account's real home, not `NSHomeDirectory()`: under the sandbox that is the
        // container, and an app run from `~/Applications` would keep the account name.
        var homes = [NSHomeDirectory()]
        if let entry = getpwuid(getuid()), let dir = entry.pointee.pw_dir {
            homes.insert(String(cString: dir), at: 0)
        }
        var out: [String] = []
        for index in 0..<_dyld_image_count() {
            guard let header = _dyld_get_image_header(index),
                let cName = _dyld_get_image_name(index)
            else { continue }
            var path = String(cString: cName)
            if let home = homes.first(where: { !$0.isEmpty && path.hasPrefix($0 + "/") }) {
                path = "~" + path.dropFirst(home.count)
            }
            let uuid = imageUUID(header) ?? "no-uuid"
            let address = String(UInt(bitPattern: header), radix: 16)
            out.append("\(uuid) 0x\(address) \(path)")
        }
        return out
    }

    private static func imageUUID(_ header: UnsafePointer<mach_header>) -> String? {
        guard header.pointee.magic == MH_MAGIC_64 else { return nil }
        var cursor = UnsafeRawPointer(header) + MemoryLayout<mach_header_64>.size
        for _ in 0..<header.pointee.ncmds {
            let command = cursor.load(as: load_command.self)
            if command.cmd == UInt32(LC_UUID) {
                let found = cursor.load(as: uuid_command.self)
                return UUID(uuid: found.uuid).uuidString
            }
            cursor += Int(command.cmdsize)
        }
        return nil
    }

    private static let fatalSignals: [Int32] = [SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT, SIGTRAP]

    /// Copy what the handler needs into memory it can read without allocating, and install it.
    ///
    /// A Rust panic that escapes D-47's catch boundaries aborts, so it arrives here as
    /// `SIGABRT`; so does a Swift runtime trap, as `SIGTRAP` or `SIGILL`.
    private static func prepareHandler(path: String, header: String) {
        crashPath = strdup(path)
        let bytes = Array(header.utf8)
        let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: bytes.count)
        buffer.initialize(from: bytes, count: bytes.count)
        crashHeader = buffer
        crashHeaderLength = bytes.count
        crashFrames = UnsafeMutablePointer<UnsafeMutableRawPointer?>.allocate(capacity: Int(maxFrames))
        crashDigits = UnsafeMutablePointer<UInt8>.allocate(capacity: 32)

        // A stack overflow leaves no stack to run a handler on, so the main thread gets one of
        // its own. Other threads' overflows are caught by the guard page and still terminate;
        // they produce the platform's report and not this one.
        let altSize = 64 * 1024
        var stack = stack_t(
            ss_sp: UnsafeMutableRawPointer.allocate(byteCount: altSize, alignment: 16),
            ss_size: altSize, ss_flags: 0)
        sigaltstack(&stack, nil)

        var action = sigaction()
        action.__sigaction_u.__sa_sigaction = crashHandler
        action.sa_flags = SA_SIGINFO | SA_RESETHAND | SA_ONSTACK
        sigemptyset(&action.sa_mask)
        for signal in fatalSignals {
            sigaction(signal, &action, nil)
        }
    }

    // MARK: - Next launch

    /// D-114's notice: the report in full, a copy saved where the user chooses, the file shown in
    /// Finder, or dismissed and deleted. Shown once per launch, and again at the next launch
    /// until it is dismissed.
    static func presentPendingReport() {
        guard let report = pendingReport else { return }
        pendingReport = nil
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
        while true {
            let alert = NSAlert()
            alert.messageText = "Sift quit unexpectedly the last time it ran"
            alert.informativeText =
                "It left the report below on this Mac. Sift never sends it anywhere: to report "
                + "the problem, save a copy or show it in Finder, and attach it to an issue "
                + "yourself. Dismissing it deletes it."
            alert.accessoryView = reportView(report)
            alert.addButton(withTitle: "Save a Copy…")
            alert.addButton(withTitle: "Show in Finder")
            alert.addButton(withTitle: "Dismiss")
            switch alert.runModal() {
            case .alertFirstButtonReturn:
                saveCopy(report)
            case .alertSecondButtonReturn:
                reveal()
                return
            default:
                if let url = reportURL { try? FileManager.default.removeItem(at: url) }
                return
            }
        }
    }

    private static func reportView(_ report: String) -> NSView {
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 560, height: 280))
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.borderType = .bezelBorder
        let text = NSTextView(frame: scroll.bounds)
        text.isEditable = false
        text.font = .monospacedSystemFont(ofSize: 10, weight: .regular)
        text.string = report
        text.isHorizontallyResizable = true
        text.textContainer?.widthTracksTextView = false
        text.textContainer?.containerSize = NSSize(
            width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        scroll.documentView = text
        return scroll
    }

    /// The platform's save panel, which is also what grants the sandboxed process the one file
    /// the user named.
    private static func saveCopy(_ report: String) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "Sift crash report.txt"
        panel.allowedContentTypes = [.plainText]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            try report.write(to: url, atomically: true, encoding: .utf8)
        } catch {
            NSAlert(error: error).runModal()
        }
    }

    // MARK: - Collecting them

    /// Show the log and any report in Finder, so a QA pass can attach them without a terminal.
    static func reveal() {
        guard let directory else { return }
        try? FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        let present = ([reportName] + logNames)
            .map { directory.appendingPathComponent($0) }
            .filter { FileManager.default.fileExists(atPath: $0.path) }
        if present.isEmpty {
            NSWorkspace.shared.open(directory)
        } else {
            NSWorkspace.shared.activateFileViewerSelecting(present)
        }
    }

    /// The log's live size on disk, against L-33's budget.
    static func logBytes() -> Int64 {
        guard let directory else { return 0 }
        return logNames.reduce(0) { total, name in
            let path = directory.appendingPathComponent(name).path
            let size = (try? FileManager.default.attributesOfItem(atPath: path)[.size]) as? Int64
            return total + (size ?? 0)
        }
    }

    static var reportExists: Bool {
        guard let url = reportURL else { return false }
        return FileManager.default.fileExists(atPath: url.path)
    }
}

// MARK: - The handler's state
//
// File-scope rather than static members, and all of it assigned in `prepareHandler` before the
// handler is installed, so the handler touches no lazily-initialised storage. Nothing below
// allocates, locks, or calls into Swift's runtime: it is `open`, `write`, `backtrace`,
// `backtrace_symbols_fd` and `close` over memory prepared at launch and the stack.

private let maxFrames: Int32 = 128
private var crashPath: UnsafeMutablePointer<CChar>?
private var crashHeader: UnsafeMutablePointer<UInt8>?
private var crashHeaderLength = 0
private var crashFrames: UnsafeMutablePointer<UnsafeMutableRawPointer?>?
private var crashDigits: UnsafeMutablePointer<UInt8>?
private var crashing: sig_atomic_t = 0
private var crashingThread: pthread_t?

private func writeStatic(_ fd: Int32, _ text: StaticString) {
    text.withUTF8Buffer { buffer in _ = write(fd, buffer.baseAddress, buffer.count) }
}

/// A number, in `radix`, without allocating.
private func writeNumber(_ fd: Int32, _ value: UInt, radix: UInt) {
    guard let digits = crashDigits else { return }
    var n = value
    var index = 32
    repeat {
        index -= 1
        let digit = UInt8(truncatingIfNeeded: n % radix)
        digits[index] = digit < 10 ? 0x30 + digit : 0x61 + digit - 10
        n /= radix
    } while n > 0 && index > 0
    _ = write(fd, digits + index, 32 - index)
}

private let crashHandler:
    @convention(c) (Int32, UnsafeMutablePointer<__siginfo>?, UnsafeMutableRawPointer?) -> Void = {
        signal, info, _ in
        // A second fatal signal while the first is being written. Returning would not keep the
        // first report: `SA_RESETHAND` has put this signal back to its default on entry, so the
        // returning thread re-faults straight into the default action and ends the process
        // mid-write, truncating the report. So another thread waits here, bounded, for the
        // crashing thread to finish and end the process itself; only if that never comes does
        // it fall through and end it. (A second thread faulting on the *same* signal never
        // reaches this handler — `SA_RESETHAND` already reset it — and ends the process
        // wherever the first report has got to.) The crashing thread faulting again inside its
        // own handler cannot wait on itself, so it ends at once and its report is as far as it
        // got. `pthread_self` reads the thread's own register and takes no lock. The claim is a plain store, not an atomic exchange — the shell targets
        // a macOS without Swift's atomics — so two threads faulting in the same instant can
        // both claim it, and then the file can hold the two reports interleaved.
        if crashing != 0 {
            if pthread_self() != crashingThread {
                var tick = timespec(tv_sec: 0, tv_nsec: 100_000_000)
                for _ in 0..<50 { nanosleep(&tick, nil) }
            }
        } else if let path = crashPath {
            crashing = 1
            crashingThread = pthread_self()
            let fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0o600)
            if fd >= 0 {
                if let header = crashHeader { _ = write(fd, header, crashHeaderLength) }
                writeStatic(fd, "signal: ")
                writeNumber(fd, UInt(signal), radix: 10)
                writeStatic(fd, "\nfault address: 0x")
                writeNumber(fd, UInt(bitPattern: info?.pointee.si_addr), radix: 16)
                writeStatic(
                    fd,
                    "\nper-subsystem counters: not captured by this build\n\nbacktrace of the thread that crashed:\n"
                )
                if let frames = crashFrames {
                    let count = backtrace(frames, maxFrames)
                    backtrace_symbols_fd(frames, count, fd)
                }
                close(fd)
            }
        }
        // The default action again, unblocked, so the process ends the way it would have —
        // including the platform's own report, which Sift cannot suppress. Returning instead
        // is not enough: a trap instruction re-executed after the handler returns was observed
        // to spin rather than terminate. `_exit` is the floor if the signal somehow does not.
        Darwin.signal(signal, SIG_DFL)
        var only = sigset_t()
        sigemptyset(&only)
        sigaddset(&only, signal)
        pthread_sigmask(SIG_UNBLOCK, &only, nil)
        raise(signal)
        _exit(128 + signal)
    }
