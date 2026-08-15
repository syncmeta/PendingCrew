#if os(macOS)
import SwiftUI
import AppKit
@preconcurrency import WebKit

/// macOS 直接登录页(扫码 / Apple / Google / 邮箱)。
///
/// VENDORED from PendingBot `apps/pendingbot/Sources/Mac/MacWelcomeView.swift` ——
/// 3-pill 布局(Apple/Google/扫码)+ 邮箱箭头行 + 「你是？」人机验证 + 机器人彩蛋
/// 逐块照搬。PendingCrew 改造:
///   1. `WelcomeViewModel` → `CrewWelcomeViewModel`;成功路径不靠 AccountStore 的
///      auth listener,而是 vm 的 `onAuthenticated` 闭包 → `CrewLoginExchange.run`
///      (临时 session access token → 家族凭据 → mint pdg_ → 存 grant → 清 session)。
///   2. **不带独立窗口 chrome**:PendingCrew 这页在 `CrewLoginSheet`(侧栏 sheet)里
///      渲染,不是 gate 窗口 —— 丢掉 `MacRootGate` / `MacLoginWindowStyler` /
///      window-sizing(`.frame(minWidth:770,...)` / 透明窗 / 红绿灯挪位),只保留
///      内容簇(logo + 表单)+ `.padding(44)`。
///   3. `MacTurnstileWebView` → `CrewMacTurnstileWebView`(repoint 到 `CrewHostedConfig`)。
///   4. logo 槽里的 `PendingBotMacQRLoginView`(mint pendingbot session)换成
///      `CrewQRLoginView`(PendingCrew device-login),扫码 pill 切换 logo↔二维码。
/// 再对齐 = 对照源文件重拷。
struct CrewMacWelcomeView: View {
    @EnvironmentObject private var model: AppModel
    @StateObject private var vm = CrewWelcomeViewModel()
    @FocusState private var focusedField: Field?
    @State private var showingQRLogin = false

    enum Field { case email, code }

    private let resendTimer = Timer.publish(every: 1, on: .main, in: .common).autoconnect()

    var body: some View {
        // Logo + form sit as one centered cluster on a compact content frame —
        // 这页在 sheet 里,没有独立窗口,所以不声明 window 尺寸,只给内容簇留白。
        HStack(spacing: 96) {
            // 左侧是个固定 140 宽的槽(=二维码宽):logo 居中、二维码填满,两者共享
            // 同一中心。切换 logo↔二维码不改变整簇宽度,所以右侧表单纹丝不动。
            Group {
                if showingQRLogin {
                    CrewQRLoginView()
                        .transition(.opacity)
                } else {
                    Image("BrandMark")
                        .resizable()
                        .interpolation(.high)
                        .antialiased(true)
                        .aspectRatio(contentMode: .fit)
                        .frame(width: 92, height: 92)
                        .clipShape(RoundedRectangle(cornerRadius: 21, style: .continuous))
                        .shadow(color: .black.opacity(0.06), radius: 18, y: 5)
                }
            }
            .frame(width: 140)

            VStack(alignment: .leading, spacing: 0) {
                if vm.stage == .humanCheck {
                    backButton
                        .padding(.bottom, 18)
                        .transition(.opacity)
                }

                switch vm.stage {
                case .enteringEmail:
                    enteringEmailBody
                case .humanCheck:
                    humanCheckBody
                        .transition(.opacity)
                }

                if let errorText = vm.errorText {
                    errorBlock(text: errorText)
                        .padding(.top, 12)
                }
            }
            .frame(width: 310)
        }
        .padding(44)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
        .background(Theme.Palette.canvas.ignoresSafeArea())
        .task { wireExchange() }
        .onChange(of: vm.otpSent) { _, sent in
            if sent { focusedField = .code }
        }
        .onReceive(resendTimer) { _ in
            vm.tickResendCooldown()
        }
        .animation(.easeInOut(duration: 0.28), value: vm.stage)
        .animation(.easeInOut(duration: 0.28), value: vm.humanChoice)
        .animation(.easeInOut(duration: 0.28), value: vm.otpSent)
        .animation(.easeInOut(duration: 0.28), value: vm.codeBlockShown)
        .animation(.easeInOut(duration: 0.28), value: vm.turnstileInteractive)
        .animation(.easeInOut(duration: 0.28), value: showingQRLogin)
    }

    // MARK: - Exchange wiring

    /// 把 vm 的 `onAuthenticated` 接到内存兑换:临时 session 的 access token →
    /// 家族凭据(pfa_)→ mint pendingcrew_control grant(pdg_)→ 存 grant + 家族凭据 →
    /// 清 supabase session。成功后 `saveDeviceGrantToken` 翻 `isAuthenticated` →
    /// `CrewLoginSheet` 自动关闭。
    private func wireExchange() {
        vm.onAuthenticated = { accessToken in
            guard let url = URL(string: model.apiBaseURL) else { return }
            let api = PendingCrewAPI(baseURL: url)
            let exchange = CrewLoginExchange(
                issueFamily: { token in
                    let r = try await api.issueFamilyCredential(accessToken: token)
                    return .init(token: r.familyCredential.token, subjectId: r.familyCredential.subjectId, displayName: r.displayName, avatarSeed: r.avatarSeed)
                },
                mintGrant: { fam, sid in
                    let resp = try await api.mintGrant(
                        familyCredential: fam, subjectId: sid,
                        grantKind: "pendingcrew_control",
                        scopes: ["subject:read", "crew:read", "crew:write", "runner:read", "runner:write"])
                    guard let token = resp.deviceGrantToken else { throw CrewLoginExchange.Failure.mintFailed }
                    return token
                },
                saveGrant: { try model.saveDeviceGrantToken($0) },
                saveFamily: { FamilyCredentialStore.set($0) },
                teardown: { try? await CrewSupabaseStack.shared.auth.signOut(scope: .local) }
            )
            try await exchange.run(accessToken: accessToken)
        }
    }

    // MARK: - Stage: entering email

    @ViewBuilder
    private var enteringEmailBody: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("登录 / 注册：")
                .font(Theme.Fonts.system(size: 15, weight: .semibold))
                .foregroundStyle(Theme.Palette.ink)
                .padding(.leading, 4)
                .padding(.bottom, 22)

            VStack(spacing: 14) {
                // Apple / Google / 扫码 三个玻璃胶囊同排:Apple 固定 110(最窄),
                // 扫码胶囊固定 56(只放一个 qrcode 图标),Google 吃掉剩余宽。三者
                // 总宽 = 下方邮箱行宽,所以输入框天然与这排对齐。
                HStack(spacing: 10) {
                    appleButton
                        .frame(width: 110)
                    googleButton
                    qrLoginButton
                        .frame(width: 56)
                }
                emailRow
            }
            .disabled(vm.isBusy)
        }
    }

    // MARK: - Stage: human check

    @ViewBuilder
    private var humanCheckBody: some View {
        VStack(alignment: .leading, spacing: 22) {
            Text("你是？")
                .font(Theme.Fonts.system(size: 26, weight: .semibold, design: .serif))
                .foregroundStyle(Theme.Palette.ink)
                .padding(.leading, 4)

            HStack(spacing: 12) {
                humanButton
                robotButton
            }
            .disabled(vm.isBusy || vm.codeBlockShown)

            turnstileWidget

            Group {
                if vm.codeBlockShown {
                    codeBlock
                        .transition(.opacity)
                } else if vm.humanChoice == .robot {
                    robotEasterEgg
                        .transition(.opacity)
                } else if vm.humanChoice == .human && !vm.turnstileInteractive && !vm.turnstileFailed {
                    verifyingHint
                        .transition(.opacity)
                }
            }
        }
    }

    private var backButton: some View {
        Button {
            vm.backToEmailEntry()
            focusedField = .email
        } label: {
            Image(systemName: "chevron.left")
                .font(Theme.Fonts.glyph(size: 16, weight: .semibold))
                .foregroundStyle(Theme.Palette.ink)
                .frame(width: 32, height: 32)
                .macGlassCircle()
                .shadow(color: .black.opacity(0.06), radius: 6, y: 2)
        }
        .buttonStyle(.plain)
        .disabled(vm.isVerifying)
        .accessibilityLabel("返回")
    }

    private var humanButton: some View {
        Button {
            vm.tapHuman()
        } label: {
            choicePill(text: "人类", filled: vm.humanChoice == .human)
        }
        .buttonStyle(.plain)
    }

    private var robotButton: some View {
        Button {
            vm.tapRobot()
        } label: {
            choicePill(text: "机器人", filled: vm.humanChoice == .robot)
        }
        .buttonStyle(.plain)
    }

    private func choicePill(text: String, filled: Bool) -> some View {
        Text(text)
            .font(Theme.Fonts.system(size: 16, weight: .medium))
            .foregroundStyle(filled ? .white : Theme.Palette.ink)
            .frame(maxWidth: .infinity)
            .frame(height: 48)
            .background(
                Group {
                    if filled {
                        Capsule().fill(Theme.Palette.accent)
                    } else {
                        Capsule().fill(Color.clear)
                    }
                }
            )
            .overlay(
                Capsule().strokeBorder(
                    filled ? Color.clear : Theme.Palette.ink.opacity(0.18),
                    lineWidth: 1
                )
            )
            .shadow(color: filled ? Color.black.opacity(0.18) : .clear, radius: 10, y: 4)
            .contentShape(Capsule())
    }

    /// Always-mounted Turnstile widget. Collapsed to a 1pt sliver for silent
    /// passes; only animates open if Cloudflare fires the
    /// `before-interactive-callback`.
    private var turnstileWidget: some View {
        let visible = vm.humanChoice == .human && !vm.codeBlockShown && vm.turnstileInteractive
        return CrewMacTurnstileWebView(
            siteKey: CrewHostedConfig.turnstileSiteKey,
            host: CrewHostedConfig.turnstileHost,
            onToken: { token in vm.handleTurnstileToken(token) },
            onError: { msg in vm.handleTurnstileError(msg) },
            onInteractive: { vm.markTurnstileInteractive() }
        )
        .frame(maxWidth: .infinity)
        .frame(height: visible ? 72 : 1)
        .opacity(visible ? 1 : 0)
        .allowsHitTesting(visible)
        .id(vm.turnstileWidgetID)
    }

    private var verifyingHint: some View {
        HStack(spacing: 8) {
            ProgressView()
                .controlSize(.small)
            Text("正在验证…")
                .font(Theme.Fonts.footnote)
                .foregroundStyle(Theme.Palette.inkMuted)
        }
        .padding(.horizontal, 4)
    }

    private var codeBlock: some View {
        VStack(spacing: 14) {
            codeStatusRow
            if vm.otpSent {
                codeRow
                    .transition(.opacity)
            }
        }
    }

    private var codeStatusRow: some View {
        HStack(spacing: 8) {
            Image(systemName: "envelope")
                .font(Theme.Fonts.glyph(size: 13))
                .foregroundStyle(Theme.Palette.inkMuted)
            Text("\(vm.otpSent ? "码已发至：" : "码将发至：")\(vm.email)")
                .font(Theme.Fonts.footnote)
                .foregroundStyle(Theme.Palette.inkMuted)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer()
            sendOrResendButton
        }
        .padding(.horizontal, 4)
    }

    private var sendOrResendButton: some View {
        Group {
            if vm.resendCooldown > 0 {
                Text("\(vm.resendCooldown)s")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.inkMuted.opacity(0.7))
            } else if !vm.otpSent {
                Button("发送") {
                    vm.tapSend()
                }
                .buttonStyle(.plain)
                .font(Theme.Fonts.caption)
                .foregroundStyle(Theme.Palette.accent.opacity(0.85))
                .disabled(vm.isVerifying || vm.turnstileToken == nil)
            } else {
                Button("重发") {
                    Task { await vm.resend() }
                }
                .buttonStyle(.plain)
                .font(Theme.Fonts.caption)
                .foregroundStyle(Theme.Palette.accent.opacity(0.85))
                .disabled(vm.isVerifying)
            }
        }
    }

    private var robotEasterEgg: some View {
        VStack(spacing: 22) {
            Text("😲😲😲")
                .font(Theme.Fonts.system(size: 56))
            VStack(spacing: 10) {
                Text("尊敬的机器人您好，请联系 hello@example.com")
                Text("我会稳稳地接住你")
                Text("我还可以帮你生成我和机器人的关系图")
            }
            .font(Theme.Fonts.footnote)
            .foregroundStyle(Theme.Palette.ink)
            .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 4)
    }

    // MARK: - Apple / Google buttons

    private var appleButton: some View {
        Button { Task { await vm.beginApple() } } label: {
            brandPillContent(logo: appleLogoImage, text: "Apple", textColor: Theme.Palette.onApplePill)
                .frame(maxWidth: .infinity)
                .frame(height: 48)
                .background(Capsule().fill(Theme.Palette.applePill))
                .shadow(color: .black.opacity(0.18), radius: 10, y: 4)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Sign in with Apple")
    }

    private var googleButton: some View {
        Button { Task { await vm.beginGoogle() } } label: {
            brandPillContent(
                logo: AnyView(Image("GoogleG").resizable().scaledToFit()),
                text: "Google",
                textColor: Theme.Palette.googleInk
            )
            .frame(maxWidth: .infinity)
            .frame(height: 48)
            .contentShape(Capsule())   // 玻璃胶囊整片可点(同 qrLoginButton)
            .macGlassCapsule()
            .shadow(color: .black.opacity(0.08), radius: 10, y: 4)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Sign in with Google")
    }

    /// 扫码登录入口——qrcode 图标装在与 Apple/Google 同款玻璃胶囊里。点击把二维码
    /// 显示在左侧 logo 的位置(见 body 的 `showingQRLogin` 分支);激活时胶囊填成
    /// accent,再点一下收起二维码 / 还原 logo(等于返回)。
    @ViewBuilder
    private var qrLoginButton: some View {
        let glyph = Image(systemName: "qrcode")
            .font(Theme.Fonts.glyph(size: 22, weight: .medium))
            .frame(maxWidth: .infinity)
            .frame(height: 48)
            // 整个胶囊都可点——否则只有 qrcode 字形本身命中,周围留白点不动。
            .contentShape(Capsule())
        Button {
            showingQRLogin.toggle()
            focusedField = nil
        } label: {
            if showingQRLogin {
                glyph
                    .foregroundStyle(Theme.Palette.onAccent)
                    .background(Capsule().fill(Theme.Palette.accent))
                    .shadow(color: .black.opacity(0.18), radius: 10, y: 4)
            } else {
                glyph
                    .foregroundStyle(Theme.Palette.ink)
                    .macGlassCapsule()
                    .shadow(color: .black.opacity(0.08), radius: 10, y: 4)
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel("扫码登录")
        .help(showingQRLogin ? "收起二维码" : "扫码登录")
    }

    /// Prefer the Apple-supplied logo-only artwork when present (shared
    /// asset catalog), falling back to the SF Symbol otherwise — mirrors iOS.
    private var appleLogoImage: AnyView {
        if NSImage(named: "AppleSignInLogo") != nil {
            AnyView(
                Image("AppleSignInLogo")
                    .resizable()
                    .renderingMode(.template)
                    .foregroundStyle(Theme.Palette.onApplePill)
                    .scaledToFit()
            )
        } else {
            AnyView(
                Image(systemName: "applelogo")
                    .resizable()
                    .scaledToFit()
                    .foregroundStyle(Theme.Palette.onApplePill)
            )
        }
    }

    private func brandPillContent(
        logo: AnyView, text: String, textColor: Color
    ) -> some View {
        HStack(spacing: 10) {
            logo.frame(width: 18, height: 18)
            Text(text)
                .font(Theme.Fonts.system(size: 18, weight: .medium))
                .foregroundStyle(textColor)
        }
    }

    // MARK: - Email / code rows

    private var emailRow: some View {
        inputRow {
            TextField("邮箱", text: $vm.email)
                .textFieldStyle(.plain)
                .focused($focusedField, equals: .email)
                .onSubmit { advance() }
        }
    }

    private var codeRow: some View {
        inputRow {
            TextField("码可能在垃圾邮件里", text: $vm.code)
                .textFieldStyle(.plain)
                .focused($focusedField, equals: .code)
                .onSubmit { Task { await vm.verifyCode() } }
        }
    }

    /// One row: a plain text input on the left, a green circular submit
    /// button on the right — same shape as the iOS inputRow.
    @ViewBuilder
    private func inputRow<Content: View>(
        @ViewBuilder field: () -> Content
    ) -> some View {
        HStack(spacing: 0) {
            field()
                .font(Theme.Fonts.system(size: 16))
                .padding(.leading, 18)
                .padding(.vertical, 13)
                .frame(maxWidth: .infinity, alignment: .leading)

            submitArrowButton
                .padding(.trailing, 7)
        }
        .frame(height: 48)
        .macGlassCapsule()
        .shadow(color: .black.opacity(0.08), radius: 10, y: 4)
    }

    private var submitArrowButton: some View {
        Button {
            switch vm.stage {
            case .enteringEmail: advance()
            case .humanCheck:    Task { await vm.verifyCode() }
            }
        } label: {
            Group {
                if vm.isBusy {
                    ProgressView()
                        .controlSize(.small)
                        .tint(Theme.Palette.onAccent)
                } else {
                    Image(systemName: "arrow.right")
                        .font(Theme.Fonts.glyph(size: 14, weight: .bold))
                        .foregroundStyle(Theme.Palette.onAccent)
                }
            }
            .frame(width: 35, height: 35)
            .background(
                Circle().fill(vm.canSubmit ? Theme.Palette.accent : Theme.Palette.accent.opacity(0.28))
            )
        }
        .buttonStyle(.plain)
        .disabled(!vm.canSubmit || vm.isBusy)
        .animation(.easeInOut(duration: 0.15), value: vm.canSubmit)
    }

    private func errorBlock(text: String) -> some View {
        Text(text)
            .font(Theme.Fonts.caption)
            .foregroundStyle(.red)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: - Actions bridging view focus → view model

    private func advance() {
        if vm.advanceToHumanCheck() {
            focusedField = nil
            showingQRLogin = false   // 走邮箱流程时把 logo 还原
        }
    }
}

/// macOS 26+ uses the Liquid Glass material via `.glassEffect`; older systems
/// fall back to the standard system material. Twin of the iOS helpers in
/// `WelcomeView`, scoped here so the Mac login reads identically.
private extension View {
    @ViewBuilder
    func macGlassCapsule() -> some View {
        if #available(macOS 26.0, *) {
            self.glassEffect(.regular, in: Capsule())
        } else {
            self
                .background(Capsule().fill(.regularMaterial))
                .overlay(Capsule().strokeBorder(Color.primary.opacity(0.06), lineWidth: 0.5))
        }
    }

    @ViewBuilder
    func macGlassCircle() -> some View {
        if #available(macOS 26.0, *) {
            self.glassEffect(.regular, in: Circle())
        } else {
            self
                .background(Circle().fill(.regularMaterial))
                .overlay(Circle().strokeBorder(Color.primary.opacity(0.06), lineWidth: 0.5))
        }
    }
}

/// macOS WKWebView host for the Cloudflare Turnstile widget — the
/// `NSViewRepresentable` twin of `CrewTurnstileWebView` (iOS). Same HTML/JS
/// bridge (`window.webkit.messageHandlers` works identically in macOS
/// WKWebView); only the representable wrapper differs. Vendored verbatim from
/// PendingBot `MacTurnstileWebView`, repointed to `CrewHostedConfig`.
struct CrewMacTurnstileWebView: NSViewRepresentable {
    let siteKey: String
    let host: URL
    var onToken: (String) -> Void
    var onError: (String) -> Void
    var onInteractive: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onToken: onToken, onError: onError, onInteractive: onInteractive)
    }

    func makeNSView(context: Context) -> WKWebView {
        let cfg = WKWebViewConfiguration()
        let userContent = WKUserContentController()
        userContent.add(context.coordinator, name: "turnstile")
        cfg.userContentController = userContent
        cfg.websiteDataStore = .nonPersistent()
        let view = WKWebView(frame: .zero, configuration: cfg)
        view.setValue(false, forKey: "drawsBackground") // 透明背景（macOS WKWebView）
        view.loadHTMLString(buildHTML(siteKey: siteKey), baseURL: host)
        return view
    }

    func updateNSView(_ view: WKWebView, context: Context) {}

    private func buildHTML(siteKey: String) -> String {
        return """
        <!doctype html>
        <html>
        <head>
          <meta charset="utf-8">
          <meta name="viewport" content="width=device-width, initial-scale=1">
          <script src="https://challenges.cloudflare.com/turnstile/v0/api.js?onload=onloadTurnstileCallback" async defer></script>
          <style>
            html, body { margin: 0; padding: 0; background: transparent; }
            body { display: flex; align-items: center; justify-content: center; min-height: 100vh; font-family: -apple-system, BlinkMacSystemFont, sans-serif; }
            #ts { display: flex; align-items: center; justify-content: center; }
          </style>
        </head>
        <body>
          <div id="ts"></div>
          <script>
            function send(type, payload) {
              try {
                window.webkit.messageHandlers.turnstile.postMessage(Object.assign({type: type}, payload || {}));
              } catch (_) {}
            }
            window.onloadTurnstileCallback = function() {
              try {
                turnstile.render('#ts', {
                  sitekey: '\(siteKey)',
                  appearance: 'interaction-only',
                  callback: function(token) { send('token', {token: token}); },
                  'error-callback': function(err) { send('error', {message: String(err)}); },
                  'expired-callback': function() { send('error', {message: 'expired'}); },
                  'timeout-callback': function() { send('error', {message: 'timeout'}); },
                  'before-interactive-callback': function() { send('interactive', {}); }
                });
              } catch (e) {
                send('error', {message: 'render: ' + (e && e.message || e)});
              }
            };
            setTimeout(function() {
              if (typeof turnstile === 'undefined') {
                send('error', {message: 'script failed to load'});
              }
            }, 10000);
          </script>
        </body>
        </html>
        """
    }

    @MainActor
    final class Coordinator: NSObject, WKScriptMessageHandler {
        let onToken: (String) -> Void
        let onError: (String) -> Void
        let onInteractive: () -> Void
        private var settled = false

        init(onToken: @escaping (String) -> Void,
             onError: @escaping (String) -> Void,
             onInteractive: @escaping () -> Void) {
            self.onToken = onToken
            self.onError = onError
            self.onInteractive = onInteractive
        }

        func userContentController(_ userContentController: WKUserContentController,
                                   didReceive message: WKScriptMessage) {
            guard let dict = message.body as? [String: Any] else { return }
            let type = dict["type"] as? String ?? ""
            switch type {
            case "token":
                guard !settled, let token = dict["token"] as? String else { return }
                settled = true
                onToken(token)
            case "error":
                guard !settled else { return }
                onError(dict["message"] as? String ?? "unknown")
            case "interactive":
                onInteractive()
            default:
                break
            }
        }
    }
}
#endif
