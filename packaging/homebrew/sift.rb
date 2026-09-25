# The Homebrew Cask — D-33, D-45, D-62.
#
# **Owner: @justin13888**, recorded in `CODEOWNERS` — the named owner this file MUST have before
# the first release. D-33 makes this the only macOS channel of the first release: the Mac App
# Store is deferred, not rejected, and is reconsidered only together with D-113's licensing
# question.
#
# This file is why two decisions elsewhere are permanent. Its zap stanza publishes the on-disk
# layout — which is what fixes D-77's blob fan-out depth at first release — and the Keychain
# service and access-group names, a change to which breaks uninstall for every existing user.

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

  # D-45 gives this build and any later App Store build **one bundle identifier and one team
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

  # `brew uninstall` **and `brew upgrade`** run this stanza, so it removes the bundle and nothing
  # the user owns. Deleting the container here would erase every account, the blob store and
  # the installation secret on each upgrade. User data belongs to `zap` alone.
  uninstall quit: "net.justinchung.sift"

  # `brew uninstall --zap`: everything Sift leaves behind, under the names this file publishes.
  # Changing any of them breaks this stanza for every existing user.
  #
  # The Keychain. Service `net.justinchung.sift` (`KEYCHAIN_SERVICE`), one generic-password item
  # per account and credential kind; access group `<team>.net.justinchung.sift` once the team
  # identifier exists (`keychain_access_group`). Individual items are not permanent — they come
  # and go with accounts — but the naming scheme is, and this is the enumeration it permits:
  # delete by service until none is left. Bounded, so an item the tool cannot remove cannot
  # spin it, and best-effort, because a Keychain the user has locked is theirs to refuse.
  zap script: {
        executable:   "/bin/sh",
        args:         [
          "-c",
          "i=0; while [ \"$i\" -lt 4096 ] && " \
          "/usr/bin/security delete-generic-password -s net.justinchung.sift >/dev/null 2>&1; " \
          "do i=$((i + 1)); done",
        ],
        must_succeed: false,
      },
      # The on-disk layout. It MUST NOT live anywhere the operating system may purge on its own:
      # a purge would remove blobs while leaving the encrypted shared blob index referencing
      # them, and the refcount rebuild covers abnormal termination rather than the OS deleting
      # files underneath a running application.
      #
      # D-45's sandboxed build keeps all of it — account databases, the shared blob index, the
      # installation policy store, and the blob store in D-77's two-level, one-byte fan-out —
      # inside its container, along with its preferences, caches and saved state. The trees are
      # removed whole, so the fan-out depth is fixed by every installed copy's store rather than
      # spelled out here. Application Support is where the same code lands outside the sandbox,
      # which is every build before the team identifier exists.
      trash:  [
        "~/Library/Application Scripts/net.justinchung.sift",
        "~/Library/Application Support/net.justinchung.sift",
        "~/Library/Caches/net.justinchung.sift",
        "~/Library/Containers/net.justinchung.sift",
        "~/Library/HTTPStorages/net.justinchung.sift",
        "~/Library/Preferences/net.justinchung.sift.plist",
        "~/Library/Saved Application State/net.justinchung.sift.savedState",
      ]
end
