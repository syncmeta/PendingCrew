// VENDORED from PendingBot apps/pendingbot/Sources/Networking/GoogleSignIn.swift
// PendingGoogleSignIn → CrewGoogleSignIn，重定向到 CrewSupabaseStack。GIDClientID
// 从 Info.plist 读 + iOS/macOS presenter 逻辑逐字照搬。PendingCrew Info.plist 的
// GIDClientID 还是 Task 9 的 YOUR_ 占位 → 在用户填真值前 run() 会抛 .missingClientID
// （预期行为）。再对齐 = 对照源文件重拷。
import Foundation
import GoogleSignIn
import Supabase
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

/// Native Google Sign-In → Supabase session.
///
/// We use the GoogleSignIn-iOS SDK (in-process, no Safari handoff) to
/// run Google's sign-in sheet and get an ID token. That JWT is handed
/// to supabase-swift's `signInWithIdToken(.google, ...)` — same shape
/// as the Apple flow.
///
/// Setup checklist:
///   • Google Cloud Console: create OAuth 2.0 Client ID, type = iOS,
///     bundle ID = com.pendingname.pendingcrew. Note the iOS Client ID
///     and the Reversed Client ID.
///   • Info.plist: register the Reversed Client ID as a URL scheme
///     (e.g. `com.googleusercontent.apps.<digits>-<token>`) under
///     CFBundleURLTypes. Also set GIDClientID = the iOS Client ID
///     (alternatively pass it programmatically — see configure()).
///   • Supabase dashboard: Authentication → Providers → Google → On.
///     Web Client ID + Secret can stay empty if you're native-only;
///     paste the iOS Client ID into "Authorized Client IDs" so
///     Supabase will accept ID tokens whose `aud` claim is that ID.
@MainActor
enum CrewGoogleSignIn {
    enum Error: Swift.Error, LocalizedError {
        case missingIdToken
        case userCancelled
        case missingClientID
        case malformedClientID(String)
        case underlying(Swift.Error)
        var errorDescription: String? {
            switch self {
            case .missingIdToken: return "Google 没返回 idToken"
            case .userCancelled: return "已取消"
            case .missingClientID: return "Info.plist 缺 GIDClientID"
            case .malformedClientID(let s):
                return "GIDClientID 格式不对：\(s)\n应填正向 (xxx.apps.googleusercontent.com)，不是反转 (com.googleusercontent.apps.xxx)"
            case .underlying(let e): return e.localizedDescription
            }
        }
    }

    /// Run the native Google sign-in sheet and exchange the resulting
    /// ID token for a Supabase session.
    static func run() async throws -> Session {
        try configureIfNeeded()

        let result: GIDSignInResult
        do {
            #if canImport(UIKit)
            guard let presenter = topViewController() else {
                throw Error.underlying(URLError(.badServerResponse))
            }
            result = try await GIDSignIn.sharedInstance.signIn(withPresenting: presenter)
            #elseif canImport(AppKit)
            // macOS: the GoogleSignIn-iOS SDK ships a native macOS target whose
            // `signIn(withPresenting:)` takes an `NSWindow` instead of a
            // `UIViewController`. Same in-process flow, no Safari handoff.
            guard let window = presentingWindow() else {
                throw Error.underlying(URLError(.badServerResponse))
            }
            result = try await GIDSignIn.sharedInstance.signIn(withPresenting: window)
            #else
            throw Error.underlying(URLError(.badServerResponse))
            #endif
        } catch let err as NSError where err.code == GIDSignInError.canceled.rawValue {
            throw Error.userCancelled
        } catch let err as Error {
            throw err
        } catch {
            throw Error.underlying(error)
        }

        guard let idToken = result.user.idToken?.tokenString else {
            throw Error.missingIdToken
        }
        do {
            return try await CrewSupabaseStack.shared.auth.signInWithIdToken(
                credentials: .init(provider: .google, idToken: idToken)
            )
        } catch {
            throw Error.underlying(error)
        }
    }

    /// Read GIDClientID from Info.plist, validate the shape, and hand
    /// it to GIDSignIn. Idempotent — the SDK caches the configuration
    /// internally so we only need to do this once.
    ///
    /// Validation: the value must be the *forward* client id ending in
    /// `.apps.googleusercontent.com`. The reversed form (used only as
    /// a URL scheme in CFBundleURLTypes) starts with
    /// `com.googleusercontent.apps.` and would silently fail to start
    /// the auth sheet — surface that mistake here with a typed error.
    private static func configureIfNeeded() throws {
        if GIDSignIn.sharedInstance.configuration != nil { return }
        let raw = Bundle.main.object(forInfoDictionaryKey: "GIDClientID") as? String ?? ""
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty || trimmed.hasPrefix("YOUR_") {
            throw Error.missingClientID
        }
        guard trimmed.hasSuffix(".apps.googleusercontent.com"),
              !trimmed.hasPrefix("com.googleusercontent.apps.") else {
            throw Error.malformedClientID(trimmed)
        }
        GIDSignIn.sharedInstance.configuration = GIDConfiguration(clientID: trimmed)
    }

    #if canImport(UIKit)
    private static func topViewController() -> UIViewController? {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        for scene in scenes {
            if let key = scene.windows.first(where: { $0.isKeyWindow })?.rootViewController {
                var top = key
                while let presented = top.presentedViewController { top = presented }
                return top
            }
        }
        return nil
    }
    #elseif canImport(AppKit)
    private static func presentingWindow() -> NSWindow? {
        NSApplication.shared.keyWindow ?? NSApplication.shared.windows.first
    }
    #endif
}
