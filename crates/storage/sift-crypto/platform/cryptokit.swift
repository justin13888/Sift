// D-75 — the macOS half of the page cipher: CryptoKit's `AES.GCM`, reached from Rust.
//
// CryptoKit is a Swift framework with no C interface, unlike the Security framework the
// credential store reaches directly. So this file is the C interface: four functions with
// C names, compiled by `build.rs` into a static archive that `page.rs` calls. It is kept
// exactly as narrow as D-75 requires — "narrow enough that a reviewer can confirm they
// agree by reading it" — which means it knows nothing about pages, counters, headers or
// key identifiers. It seals and opens one AEAD message under a stated nonce and stated
// additional data, and every byte of the format around that stays in `page.rs`, shared by
// both platforms.
//
// The contract each function keeps with `page.rs`:
//
// - The key is 32 bytes, the nonce 12, the tag 16. Nothing else is accepted.
// - `seal` writes exactly `msgLen + 16` bytes to `out`: ciphertext, then tag. That is the
//   layout the vendored construction produces and the byte-for-byte fixture pins.
// - `open` writes exactly `ctLen - 16` bytes to `out`, and only after the tag verified.
// - Every failure is a non-zero return. None is distinguished: an authentication failure
//   must not be told apart from any other refusal (see `PageError::FailedAuthentication`).
//
// Nothing here throws across the boundary, allocates on behalf of the caller, or keeps a
// reference past its return except the key handle, which the caller frees exactly once.

import CryptoKit
import Foundation

/// The key, held by CryptoKit's own storage — which zeroes itself on release — for as long
/// as the Rust side's `PageCipher` lives. Immutable after construction, so one handle may be
/// used from several threads at once.
private final class SiftPageKey {
    let key: SymmetricKey
    init(_ key: SymmetricKey) { self.key = key }
}

private let tagLength = 16
private let nonceLength = 12
private let keyLength = 32

@_cdecl("sift_cryptokit_key_new")
public func siftCryptoKitKeyNew(_ bytes: UnsafePointer<UInt8>, _ length: Int) -> UnsafeMutableRawPointer? {
    guard length == keyLength else { return nil }
    let key = SymmetricKey(data: UnsafeRawBufferPointer(start: bytes, count: length))
    return Unmanaged.passRetained(SiftPageKey(key)).toOpaque()
}

@_cdecl("sift_cryptokit_key_free")
public func siftCryptoKitKeyFree(_ handle: UnsafeMutableRawPointer) {
    Unmanaged<SiftPageKey>.fromOpaque(handle).release()
}

@_cdecl("sift_cryptokit_seal")
public func siftCryptoKitSeal(
    _ handle: UnsafeRawPointer,
    _ nonce: UnsafePointer<UInt8>, _ nonceLen: Int,
    _ aad: UnsafePointer<UInt8>, _ aadLen: Int,
    _ msg: UnsafePointer<UInt8>, _ msgLen: Int,
    _ out: UnsafeMutablePointer<UInt8>, _ outLen: Int
) -> Int32 {
    guard nonceLen == nonceLength, msgLen >= 0, outLen == msgLen + tagLength else { return 1 }
    let key = Unmanaged<SiftPageKey>.fromOpaque(handle).takeUnretainedValue().key
    do {
        let n = try AES.GCM.Nonce(data: UnsafeRawBufferPointer(start: nonce, count: nonceLen))
        let box = try AES.GCM.seal(
            UnsafeRawBufferPointer(start: msg, count: msgLen),
            using: key,
            nonce: n,
            authenticating: UnsafeRawBufferPointer(start: aad, count: aadLen)
        )
        guard box.ciphertext.count == msgLen, box.tag.count == tagLength else { return 1 }
        box.ciphertext.copyBytes(to: out, count: msgLen)
        box.tag.copyBytes(to: out + msgLen, count: tagLength)
        return 0
    } catch {
        return 1
    }
}

@_cdecl("sift_cryptokit_open")
public func siftCryptoKitOpen(
    _ handle: UnsafeRawPointer,
    _ nonce: UnsafePointer<UInt8>, _ nonceLen: Int,
    _ aad: UnsafePointer<UInt8>, _ aadLen: Int,
    _ sealed: UnsafePointer<UInt8>, _ sealedLen: Int,
    _ out: UnsafeMutablePointer<UInt8>, _ outLen: Int
) -> Int32 {
    guard nonceLen == nonceLength, sealedLen >= tagLength, outLen == sealedLen - tagLength else {
        return 1
    }
    let key = Unmanaged<SiftPageKey>.fromOpaque(handle).takeUnretainedValue().key
    do {
        let n = try AES.GCM.Nonce(data: UnsafeRawBufferPointer(start: nonce, count: nonceLen))
        let box = try AES.GCM.SealedBox(
            nonce: n,
            ciphertext: UnsafeRawBufferPointer(start: sealed, count: outLen),
            tag: UnsafeRawBufferPointer(start: sealed + outLen, count: tagLength)
        )
        let plain = try AES.GCM.open(
            box,
            using: key,
            authenticating: UnsafeRawBufferPointer(start: aad, count: aadLen)
        )
        guard plain.count == outLen else { return 1 }
        plain.copyBytes(to: out, count: outLen)
        return 0
    } catch {
        return 1
    }
}
