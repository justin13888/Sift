# The Homebrew Cask — D-33, D-45, D-62.
#
# **Needs a named owner in this repository before the first release.** D-33 makes this one of
# two macOS channels from the first release rather than a later convenience: the App Store
# reaches everyone else, and this reaches the technically-inclined half.
#
# This file is why two decisions elsewhere are permanent. Its uninstall stanza publishes the
# on-disk layout — which is what fixes D-77's blob fan-out depth at first release — and the
# Keychain service and access-group names, a change to which breaks uninstall for every
# existing user.

cask "sift" do
  version "0.1.0"
  sha256 :no_check

  url "https://github.com/justin13888/Sift/releases/download/v#{version}/Sift-#{version}.dmg"
  name "Sift"
  desc "Desktop mail client for reading, searching, and triaging mail"
  homepage "https://github.com/justin13888/Sift"

  # D-46's floor is structural rather than conservative: the modern sandbox-legal
  # background-residency mechanism registers the main application, while the older one needs a
  # separate helper executable inside the bundle — a second process, which would make D-2
  # false on the App Store channel.
  depends_on macos: ">= :ventura"

  app "Sift.app"

  # D-45 gives this build and the App Store build **one bundle identifier and one team
  # identifier, both sandboxed**. Two identifiers would mean two containers, two
  # per-installation secrets, and under D-43 a channel switch that discards the blob cache and
  # re-authenticates every account. The accepted cost is that the two cannot be installed side
  # by side.
  #
  # This build is not reviewed by the platform, so it carries its own signing path: a
  # distribution certificate under the same team identifier, the hardened runtime, and
  # **notarization with stapling** — so a first launch does not require the user to be online.
  #
  # The hardened runtime is load-bearing rather than hygiene: the threat model puts an
  # in-address-space attacker out of scope *because the platform prevents it*, and an
  # entitlement re-permitting debugger attachment would move a whole class of attack into
  # scope silently.

  uninstall quit: "net.justinchung.sift",
            delete: [
              "~/Library/Containers/net.justinchung.sift",
            ]

  # The on-disk layout, published here and therefore permanent. It MUST NOT live anywhere the
  # operating system may purge on its own: a purge would remove blobs while leaving the
  # encrypted shared blob index referencing them, and the refcount rebuild covers abnormal
  # termination rather than the OS deleting files underneath a running application.
  zap trash: [
    "~/Library/Containers/net.justinchung.sift",
    "~/Library/Application Support/net.justinchung.sift",
  ],
      # The Keychain service name. Published here, so changing it breaks uninstall for
      # existing users — individual credential items are not permanent, but the naming scheme
      # is.
      signal: ["TERM", "net.justinchung.sift"]
end
