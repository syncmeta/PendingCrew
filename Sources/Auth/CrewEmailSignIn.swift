// VENDORED from PendingBot apps/pendingbot/Sources/Networking/EmailSignIn.swift
// 仅取 EmailSignIn enum（requestCode/verify），重定向到 CrewSupabaseStack。
// verifyTokenHash / PendingBotDeviceLoginAPI 不搬 —— PendingCrew 自己的设备登录
// 走 PendingCrewAPI。再对齐 = 对照源文件重拷。
import Foundation
import Supabase
#if os(iOS)
import UIKit
#endif

/// Email-based sign-in via Supabase OTP.
///
/// Two-step flow:
///   1. `requestCode(email:)` — Supabase sends a 6-digit code to the
///      address. The first time this triggers `shouldCreateUser=true`
///      so the user row is provisioned on the way through.
///   2. `verify(email:, code:)` — exchange the code for a session.
///
/// Server-side prerequisite: Supabase project → Authentication →
/// Providers → Email is enabled (default on). Email templates can stay
/// at the supabase defaults; the OTP code is rendered in the {{Token}}
/// placeholder of the magic-link template by default.
@MainActor
enum CrewEmailSignIn {
    enum Error: Swift.Error, LocalizedError {
        case underlying(Swift.Error)
        var errorDescription: String? {
            switch self {
            case .underlying(let e): return e.localizedDescription
            }
        }
    }

    /// Send the OTP. Throws if the address is malformed or rate-limited.
    ///
    /// `captchaToken` is the Cloudflare Turnstile token obtained from
    /// `CrewTurnstileWebView`. Required when the Supabase project has
    /// captcha protection enabled (local dev: `supabase/config.toml`,
    /// remote: Dashboard → Auth). Pass `nil` only for tests / local
    /// runs where captcha is intentionally off.
    static func requestCode(email: String, captchaToken: String? = nil) async throws {
        do {
            try await CrewSupabaseStack.shared.auth.signInWithOTP(
                email: email,
                shouldCreateUser: true,
                captchaToken: captchaToken
            )
        } catch {
            throw Error.underlying(error)
        }
    }

    /// Exchange the 6-digit code for a session. supabase-swift returns
    /// an AuthResponse whose .session is non-nil after a successful
    /// email verifyOTP — pull that out for the caller's convenience.
    static func verify(email: String, code: String) async throws -> Session {
        do {
            let response = try await CrewSupabaseStack.shared.auth.verifyOTP(
                email: email,
                token: code,
                type: .email
            )
            if let session = response.session {
                return session
            }
            // Pull whatever the auth client persisted as a fallback.
            return try await CrewSupabaseStack.shared.auth.session
        } catch {
            throw Error.underlying(error)
        }
    }
}
