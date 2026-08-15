// VENDORED from PendingBot apps/pendingbot/Sources/Networking/AppleSignIn.swift
// AppleSignIn → CrewAppleSignIn，重定向到 CrewSupabaseStack。跨平台 SIWA 逻辑
// （ASAuthorizationController + NSWindow/UIWindow anchor + 内联 SHA-256）逐字照搬。
// 再对齐 = 对照源文件重拷。
import Foundation
import AuthenticationServices
import Supabase
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// Sign in with Apple → Supabase session.
///
/// **Cross-platform:** `ASAuthorizationController` is available on both iOS
/// and macOS — only the presentation anchor differs (a `UIWindow` on iOS, an
/// `NSWindow` on macOS). The whole flow below compiles on both; the anchor
/// lookup branches on `canImport(UIKit)` vs `canImport(AppKit)`.
///
/// We hand Apple's identity token to supabase-swift's `signInWithIdToken`,
/// which verifies the JWT against Apple's JWKS server-side and returns a
/// Supabase session bound to a user row in auth.users.
///
/// Apple supplies a `nonce` for replay protection. We generate a random
/// string, send the SHA256 of it as `request.nonce`, and pass the *raw*
/// (un-hashed) value to Supabase — Supabase recomputes the hash and checks
/// against the JWT claim.
@MainActor
enum CrewAppleSignIn {
    enum Error: Swift.Error, LocalizedError {
        case missingIdentityToken
        case userCancelled
        case underlying(Swift.Error)
        var errorDescription: String? {
            switch self {
            case .missingIdentityToken: return "Apple 没有返回 identity token"
            case .userCancelled: return "已取消"
            case .underlying(let e): return e.localizedDescription
            }
        }
    }

    /// Run the ASAuthorizationController flow and exchange the result with
    /// Supabase. In PendingCrew the session lands in CrewSupabaseStack's
    /// in-memory storage (never keychain); the caller hands its accessToken
    /// to CrewLoginExchange and then signs out, so it's one-time only.
    static func run() async throws -> Session {
        let nonce = randomNonce()
        let hashedNonce = sha256(nonce)

        let request = ASAuthorizationAppleIDProvider().createRequest()
        request.requestedScopes = [.fullName, .email]
        request.nonce = hashedNonce

        let credential: ASAuthorizationAppleIDCredential = try await withCheckedThrowingContinuation { cont in
            let controller = ASAuthorizationController(authorizationRequests: [request])
            let delegate = CrewAppleSignInDelegate(continuation: cont)
            controller.delegate = delegate
            controller.presentationContextProvider = delegate
            // Hold the delegate alive for the duration of the controller call;
            // ASAuthorizationController holds only a weak reference.
            objc_setAssociatedObject(controller, &CrewAppleSignInDelegate.holderKey, delegate, .OBJC_ASSOCIATION_RETAIN)
            controller.performRequests()
        }

        guard let tokenData = credential.identityToken,
              let idTokenString = String(data: tokenData, encoding: .utf8) else {
            throw Error.missingIdentityToken
        }

        let session = try await CrewSupabaseStack.shared.auth.signInWithIdToken(
            credentials: .init(provider: .apple, idToken: idTokenString, nonce: nonce)
        )
        return session
    }

    // MARK: - Helpers

    private static func randomNonce(length: Int = 32) -> String {
        precondition(length > 0)
        var bytes = [UInt8](repeating: 0, count: length)
        let status = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        precondition(status == errSecSuccess, "SecRandomCopyBytes failed: \(status)")
        // URL-safe printable subset, fits Apple's allowed character set.
        let charset: [Character] = Array("0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-._")
        return String(bytes.map { charset[Int($0) % charset.count] })
    }

    private static func sha256(_ input: String) -> String {
        let bytes = Array(input.utf8)
        var hash = [UInt8](repeating: 0, count: 32)
        // Lightweight inline SHA-256 to avoid pulling CommonCrypto headers
        // into a Swift Package context. Block-sized; input is short (<256B).
        crewSha256Compute(bytes, into: &hash)
        return hash.map { String(format: "%02x", $0) }.joined()
    }
}

// MARK: - ASAuthorizationController plumbing

@MainActor
private final class CrewAppleSignInDelegate: NSObject, ASAuthorizationControllerDelegate, ASAuthorizationControllerPresentationContextProviding {
    nonisolated(unsafe) static var holderKey: UInt8 = 0

    let continuation: CheckedContinuation<ASAuthorizationAppleIDCredential, Swift.Error>

    init(continuation: CheckedContinuation<ASAuthorizationAppleIDCredential, Swift.Error>) {
        self.continuation = continuation
    }

    nonisolated func authorizationController(controller: ASAuthorizationController,
                                             didCompleteWithAuthorization authorization: ASAuthorization) {
        if let cred = authorization.credential as? ASAuthorizationAppleIDCredential {
            continuation.resume(returning: cred)
        } else {
            continuation.resume(throwing: CrewAppleSignIn.Error.missingIdentityToken)
        }
    }

    nonisolated func authorizationController(controller: ASAuthorizationController,
                                             didCompleteWithError error: Swift.Error) {
        if let asError = error as? ASAuthorizationError, asError.code == .canceled {
            continuation.resume(throwing: CrewAppleSignIn.Error.userCancelled)
        } else {
            continuation.resume(throwing: CrewAppleSignIn.Error.underlying(error))
        }
    }

    nonisolated func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor {
        // ASAuthorizationController guarantees this is called on main, so it
        // is safe to assume MainActor isolation for the window lookup.
        MainActor.assumeIsolated {
            #if canImport(UIKit)
            let scenes = UIApplication.shared.connectedScenes
                .compactMap { $0 as? UIWindowScene }
            for scene in scenes {
                if let window = scene.windows.first(where: { $0.isKeyWindow }) {
                    return window
                }
            }
            return ASPresentationAnchor()
            #elseif canImport(AppKit)
            return NSApplication.shared.keyWindow
                ?? NSApplication.shared.windows.first
                ?? ASPresentationAnchor()
            #else
            return ASPresentationAnchor()
            #endif
        }
    }
}

// MARK: - SHA-256

// Standard FIPS-180-4 implementation, scoped to the bytes we'll see (a 32-byte
// nonce string). Uses unchecked overflow ops so we don't dance around UInt32
// modular arithmetic in Swift.
private func crewSha256Compute(_ message: [UInt8], into out: inout [UInt8]) {
    let k: [UInt32] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ]
    var h: [UInt32] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]

    var padded = message
    let bitLen = UInt64(message.count) * 8
    padded.append(0x80)
    while padded.count % 64 != 56 { padded.append(0) }
    for i in (0..<8).reversed() {
        padded.append(UInt8((bitLen >> UInt64(i * 8)) & 0xff))
    }

    var w = [UInt32](repeating: 0, count: 64)
    for chunk in stride(from: 0, to: padded.count, by: 64) {
        for i in 0..<16 {
            let b0 = UInt32(padded[chunk + i*4 + 0]) << 24
            let b1 = UInt32(padded[chunk + i*4 + 1]) << 16
            let b2 = UInt32(padded[chunk + i*4 + 2]) << 8
            let b3 = UInt32(padded[chunk + i*4 + 3])
            w[i] = b0 | b1 | b2 | b3
        }
        for i in 16..<64 {
            let s0 = (w[i-15] >> 7 | w[i-15] << 25) ^ (w[i-15] >> 18 | w[i-15] << 14) ^ (w[i-15] >> 3)
            let s1 = (w[i-2] >> 17 | w[i-2] << 15) ^ (w[i-2] >> 19 | w[i-2] << 13) ^ (w[i-2] >> 10)
            w[i] = w[i-16] &+ s0 &+ w[i-7] &+ s1
        }
        var a = h[0], b = h[1], c = h[2], d = h[3]
        var e = h[4], f = h[5], g = h[6], hh = h[7]
        for i in 0..<64 {
            let S1 = (e >> 6 | e << 26) ^ (e >> 11 | e << 21) ^ (e >> 25 | e << 7)
            let ch = (e & f) ^ (~e & g)
            let t1 = hh &+ S1 &+ ch &+ k[i] &+ w[i]
            let S0 = (a >> 2 | a << 30) ^ (a >> 13 | a << 19) ^ (a >> 22 | a << 10)
            let mj = (a & b) ^ (a & c) ^ (b & c)
            let t2 = S0 &+ mj
            hh = g; g = f; f = e; e = d &+ t1
            d = c; c = b; b = a; a = t1 &+ t2
        }
        h[0] &+= a; h[1] &+= b; h[2] &+= c; h[3] &+= d
        h[4] &+= e; h[5] &+= f; h[6] &+= g; h[7] &+= hh
    }
    for i in 0..<8 {
        out[i*4 + 0] = UInt8((h[i] >> 24) & 0xff)
        out[i*4 + 1] = UInt8((h[i] >> 16) & 0xff)
        out[i*4 + 2] = UInt8((h[i] >> 8) & 0xff)
        out[i*4 + 3] = UInt8(h[i] & 0xff)
    }
}
