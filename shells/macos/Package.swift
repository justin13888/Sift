// swift-tools-version: 5.9
//
// D-61 — **the platform's own application toolchain owns the bundle**, and the Rust core is
// built into a static library it links. The Rust workspace never produces the `.app`.
//
// Static rather than dynamic, for reasons that are about signing rather than size: a dynamic
// library would be a second signed artefact with its own load path and hardened-runtime
// posture, and it would stop D-59's ABI leaf from being genuinely a leaf — one archive, one
// symbol surface, nothing resolved at runtime.
//
// The cost D-61 accepts: **the macOS build is not reproducible from `cargo build` alone**, a
// contributor needs the platform toolchain, and CI needs a macOS host for even a smoke build.

import PackageDescription

let package = Package(
    name: "SiftShell",
    platforms: [
        // D-46's floor, and it is structural rather than conservative: the modern
        // sandbox-legal background-residency mechanism registers the *main application*,
        // while the older one requires a separate helper executable inside the bundle — a
        // second process, which would make D-2 false on the App Store channel.
        .macOS(.v13)
    ],
    targets: [
        .systemLibrary(name: "CSift", path: "Sources/CSift"),
        .executableTarget(
            name: "SiftShell",
            dependencies: ["CSift"],
            path: "Sources/SiftShell",
            linkerSettings: [
                .unsafeFlags([
                    "-L../../target/release",
                    "-L../../target/debug",
                    "-lsift_abi",
                ])
            ]
        ),
    ]
)
