// VENDORED from PendingBot apps/pendingbot/Sources/Features/Onboarding/WelcomeViewModel.swift
// WelcomeViewModel → CrewWelcomeViewModel。Turnstile / stage / cooldown 状态机逐字照搬。
// 两处 PendingCrew 改造：
//   1. 登录 helper 重命名 EmailSignIn/AppleSignIn/PendingGoogleSignIn →
//      CrewEmailSignIn/CrewAppleSignIn/CrewGoogleSignIn。
//   2. 成功路径不再依赖 AccountStore 的 auth listener（PendingCrew 没有）——
//      beginApple/beginGoogle/verifyCode 捕获返回的 Session，把 accessToken
//      交给注入的 onAuthenticated 闭包（视图装配成 CrewLoginExchange.run）。
// 再对齐 = 对照源文件重拷。
import SwiftUI

/// Shared sign-in state machine for the signed-out screen, driving BOTH the
/// iOS `WelcomeView` and the native macOS `MacWelcomeView`. The two platforms
/// render the same flow with platform-native controls (UIKit vs AppKit text
/// fields, a `UIViewRepresentable` vs `NSViewRepresentable` Turnstile webview,
/// glass material vs material fallback) but share every piece of *logic* here
/// so the email-OTP / Apple / Google flows can't drift between platforms.
///
/// Flow: email entry → "你是？" human-check (pre-loads a Cloudflare Turnstile
/// token in the background) → code entry once the OTP has been sent. Apple and
/// Google go straight through `CrewAppleSignIn` / `CrewGoogleSignIn` (both now
/// cross-platform). The view layer owns only `@FocusState` and the actual
/// representable widget; it reacts to published state via `onChange`.
@MainActor
final class CrewWelcomeViewModel: ObservableObject {
    enum Stage {
        case enteringEmail
        case humanCheck
    }

    enum HumanChoice {
        case undecided
        case human
        case robot
    }

    /// 拿到一次性 session 的 access token 后交给上层（视图装配成 CrewLoginExchange.run）。
    var onAuthenticated: ((String) async throws -> Void)?

    @Published var stage: Stage = .enteringEmail
    /// Orthogonal to `stage`: true during any in-flight auth round-trip
    /// (email-code verify, Apple SIWA token exchange, Google sign-in).
    /// Kept separate from `stage` so Apple/Google verification doesn't
    /// flip the UI into the email-code step.
    @Published var isVerifying: Bool = false
    @Published var email: String = ""
    @Published var code: String = ""

    // MARK: Human-check stage state

    @Published var humanChoice: HumanChoice = .undecided
    /// Latest Turnstile token. Captured silently as soon as Cloudflare
    /// emits one; cleared once consumed by `sendCode`.
    @Published var turnstileToken: String?
    /// True between "user tapped 人类" and "we issued a sendCode call" —
    /// flips back to false the moment a token is consumed. Lets the
    /// async token callback know it should fire sendCode immediately.
    @Published var awaitingTurnstileSend: Bool = false
    /// Cloudflare actually wants the user to interact (rare). Until this
    /// flips true, the widget stays collapsed and the user only sees a
    /// "正在验证…" affordance.
    @Published var turnstileInteractive: Bool = false
    /// Set on hard widget failure (script load timeout, expired, etc).
    @Published var turnstileFailed: Bool = false
    /// Bumping this id remounts the underlying webview so we get a fresh
    /// token — used on resend, since Turnstile tokens are single-use.
    @Published var turnstileWidgetID: UUID = UUID()
    /// True after the first successful `sendCode`. Used to flip the
    /// status row from "码将发至" / 发送 → "码已发至" / 重发, and to
    /// reveal the code-entry input.
    @Published var otpSent: Bool = false
    /// Seconds remaining on the resend cooldown. >0 ⇒ button is the
    /// grey countdown; ==0 ⇒ "重发" is tappable again.
    @Published var resendCooldown: Int = 0
    @Published var errorText: String?

    // MARK: - Derived state

    /// True once we're ready to surface the verify-code block: Turnstile
    /// has handed us a token (we just need the user to tap 发送), an OTP
    /// request is already in flight, or one has been sent at least once.
    var codeBlockShown: Bool {
        humanChoice == .human && (turnstileToken != nil || otpSent || isVerifying)
    }

    var isBusy: Bool { isVerifying }

    /// Whether the email address looks well-formed enough to advance.
    var emailLooksValid: Bool {
        email.contains("@") && email.contains(".")
    }

    /// Whether the trailing arrow / primary button can fire in the current stage.
    var canSubmit: Bool {
        if isVerifying { return false }
        switch stage {
        case .enteringEmail:
            return emailLooksValid
        case .humanCheck:
            return otpSent && code.count >= 4
        }
    }

    var trimmedEmail: String {
        email.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    }

    // MARK: - Resend cooldown

    /// Driven by the view's 1s timer.
    func tickResendCooldown() {
        if resendCooldown > 0 { resendCooldown -= 1 }
    }

    // MARK: - Stage transitions

    func backToEmailEntry() {
        if isVerifying { return }
        errorText = nil
        humanChoice = .undecided
        turnstileToken = nil
        awaitingTurnstileSend = false
        turnstileInteractive = false
        turnstileFailed = false
        otpSent = false
        code = ""
        resendCooldown = 0
        // Drop the webview so it stops loading; it'll be remounted with
        // a fresh id next time the user advances.
        turnstileWidgetID = UUID()
        stage = .enteringEmail
    }

    /// 云端后端没配（仓库只带占位坐标）时，在**用户按下去那一刻**说清楚，
    /// 而不是让他走到 Turnstile / 发码那一步撞一堵沉默的墙。
    ///
    /// 只拦用户主动发起的三条登录入口（邮箱 / Apple / Google），**不在启动
    /// 路径上做任何事** —— 只想跑本地 crew 的人从头到尾不该被这件事打扰。
    /// 判据是 `CrewHostedConfig.isConfigured`，唯一真值，填上真坐标那天这里
    /// 自动不再拦。
    private func hostedBackendUnavailable() -> Bool {
        if CrewHostedConfig.isConfigured { return false }
        errorText = CrewHostedConfig.unconfiguredNotice
        Haptics.error()
        return true
    }

    /// Returns true if it actually advanced (email was valid).
    @discardableResult
    func advanceToHumanCheck() -> Bool {
        guard emailLooksValid else { return false }
        if hostedBackendUnavailable() { return false }
        errorText = nil
        humanChoice = .undecided
        turnstileToken = nil
        awaitingTurnstileSend = false
        turnstileInteractive = false
        turnstileFailed = false
        otpSent = false
        // New widget instance ⇒ fresh token. Cheap because the webview
        // hasn't been mounted yet (we're about to enter humanCheck).
        turnstileWidgetID = UUID()
        stage = .humanCheck
        return true
    }

    func tapHuman() {
        if codeBlockShown { return }
        humanChoice = .human
        errorText = nil
        if turnstileFailed {
            // Force a fresh widget — the previous one already errored.
            turnstileFailed = false
            turnstileInteractive = false
            turnstileToken = nil
            turnstileWidgetID = UUID()
        }
    }

    func tapRobot() {
        humanChoice = .robot
    }

    // MARK: - Turnstile callbacks

    func handleTurnstileToken(_ token: String) {
        turnstileToken = token
        // Resend path: the user has already implicitly authorized a send,
        // so consume the fresh token immediately.
        if awaitingTurnstileSend {
            awaitingTurnstileSend = false
            turnstileToken = nil
            Task { await sendCode(captchaToken: token) }
        }
    }

    func handleTurnstileError(_ msg: String) {
        turnstileFailed = true
        turnstileInteractive = false
        // Drop any prior token — error/expired callbacks invalidate it,
        // and we don't want the 发送 button to remain tappable with a
        // stale value.
        turnstileToken = nil
        if humanChoice == .human {
            errorText = "人机验证失败：\(msg)"
            Haptics.error()
        }
        awaitingTurnstileSend = false
    }

    func markTurnstileInteractive() {
        turnstileInteractive = true
    }

    /// First-tap 发送 in the status row: consume the Turnstile token already
    /// in hand and fire the initial OTP.
    func tapSend() {
        guard let token = turnstileToken else { return }
        turnstileToken = nil
        // Flip isVerifying synchronously so SwiftUI's next render already
        // sees an in-flight state — otherwise there's a one-frame gap where
        // the code block falls back to "正在验证…".
        isVerifying = true
        Task { await sendCode(captchaToken: token) }
    }

    // MARK: - Email-code flow

    func sendCode(captchaToken: String) async {
        isVerifying = true
        defer { isVerifying = false }
        do {
            try await CrewEmailSignIn.requestCode(
                email: trimmedEmail,
                captchaToken: captchaToken
            )
            otpSent = true
            resendCooldown = 60
            Haptics.tap()
        } catch {
            errorText = error.localizedDescription
            Haptics.error()
        }
    }

    func resend() async {
        if resendCooldown > 0 || isVerifying { return }
        errorText = nil
        // Fresh Turnstile token: bump widget id, then either reuse a
        // newly-arrived token or wait for one (revealing the widget if
        // CF demands an interaction).
        turnstileToken = nil
        turnstileInteractive = false
        turnstileFailed = false
        turnstileWidgetID = UUID()
        awaitingTurnstileSend = true
    }

    func verifyCode() async {
        guard otpSent else { return }
        isVerifying = true
        defer { isVerifying = false }
        do {
            let session = try await CrewEmailSignIn.verify(
                email: trimmedEmail,
                code: code.trimmingCharacters(in: .whitespaces)
            )
            try await onAuthenticated?(session.accessToken)
            Haptics.success()
        } catch {
            errorText = error.localizedDescription
            Haptics.error()
        }
    }

    // MARK: - Apple / Google

    func beginApple() async {
        errorText = nil
        if hostedBackendUnavailable() { return }
        isVerifying = true
        defer { isVerifying = false }
        do {
            let session = try await CrewAppleSignIn.run()
            try await onAuthenticated?(session.accessToken)
            Haptics.success()
        } catch CrewAppleSignIn.Error.userCancelled {
            // user backed out of the SIWA sheet — silent, stay put
        } catch {
            errorText = error.localizedDescription
            Haptics.error()
        }
    }

    func beginGoogle() async {
        errorText = nil
        if hostedBackendUnavailable() { return }
        isVerifying = true
        defer { isVerifying = false }
        do {
            let session = try await CrewGoogleSignIn.run()
            try await onAuthenticated?(session.accessToken)
            Haptics.success()
        } catch CrewGoogleSignIn.Error.userCancelled {
            // user backed out of the Google sheet — silent, stay put
        } catch {
            errorText = error.localizedDescription
            Haptics.error()
        }
    }
}
