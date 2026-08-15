import SwiftUI
import CoreImage
import CoreImage.CIFilterBuiltins

#if canImport(AppKit)
import AppKit
#elseif canImport(UIKit)
import UIKit
#endif

/// PendingCrew 设备登录(扫码)的自包含内容视图。
///
/// 从旧 `LoginView` 抽出的 device-login QR 流程,**不带任何外层 chrome**(header /
/// 标题 / 整页布局都不画)——调用方负责摆放:
/// - Mac:塞进 `CrewMacWelcomeView` 左侧 logo 槽(扫码 pill 切换 logo↔二维码)。
/// - iPad:`CrewWelcomeView` 的扫码 pill 弹出(inline 展开 / sheet)。
///
/// 流程(spec v2 §4.4,与旧 LoginView 成功分支逐字一致):
/// 1. `POST /v1/device-login/challenges`(`appKind = pendingcrew_macos`,不传
///    subjectId → 让 PendingBot 端选 subject)
/// 2. 渲染 QR + 验证码;每 2s `GET /v1/device-login/challenges/:id` poll
/// 3. PendingBot 用户扫码 + 选 subject + 批准 → poll 拿到 `deviceGrantToken`
///    → `model.saveDeviceGrantToken(token)` + 家族凭据落共享 keychain
/// 4. `isAuthenticated` 翻 true → RootView 自动切回主界面 / 登录 sheet 关闭
struct CrewQRLoginView: View {
    @EnvironmentObject private var model: AppModel
    @State private var challenge: DeviceLoginChallenge?
    @State private var status: Status = .idle
    @State private var pollTask: Task<Void, Never>?
    @State private var errorMessage: String?

    enum Status: Equatable {
        case idle
        case requesting
        case waitingForApproval
        case approved
        case error
    }

    var body: some View {
        VStack(spacing: 16) {
            switch status {
            case .idle:
                ProgressView()
                    .controlSize(.regular)
            case .requesting:
                ProgressView("正在生成登录二维码…")
                    .controlSize(.regular)
                    .font(Theme.Fonts.footnote)
            case .waitingForApproval:
                challengeSection
            case .approved:
                VStack(spacing: 8) {
                    Image(systemName: "checkmark.seal.fill")
                        .font(.system(size: 44))
                        .foregroundStyle(.green)
                    Text("登录成功，正在进入…")
                        .font(Theme.Fonts.footnote)
                        .foregroundStyle(Theme.Palette.inkMuted)
                }
            case .error:
                errorSection
            }
        }
        .frame(maxWidth: .infinity)
        .task { start() }
        .onDisappear { pollTask?.cancel() }
    }

    // MARK: - Sections

    private var challengeSection: some View {
        guard let challenge else { return AnyView(EmptyView()) }
        return AnyView(
            VStack(spacing: 14) {
                if let image = qrImage(payload: challenge.qrPayload) {
                    image
                        .resizable()
                        .interpolation(.none)
                        .scaledToFit()
                        .frame(width: 200, height: 200)
                        .padding(8)
                        .background(.white)
                        .cornerRadius(12)
                        .shadow(color: .black.opacity(0.08), radius: 12, y: 4)
                } else {
                    Text("无法生成二维码")
                        .font(Theme.Fonts.footnote)
                        .foregroundStyle(.secondary)
                }
                VStack(spacing: 4) {
                    Text("或在 PendingBot 输入验证码")
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.inkMuted)
                    Text(challenge.code)
                        .font(Theme.Fonts.monospaced(size: 20, weight: .semibold))
                        .foregroundStyle(Theme.Palette.ink)
                        .tracking(2)
                }
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                    Text("等待 PendingBot 端批准（4 分钟内有效）")
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.inkMuted)
                }
            }
        )
    }

    private var errorSection: some View {
        VStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 32))
                .foregroundStyle(.orange)
            Text(errorMessage ?? "登录失败")
                .font(Theme.Fonts.footnote)
                .multilineTextAlignment(.center)
                .foregroundStyle(Theme.Palette.inkMuted)
            Button("重试") { start() }
                .buttonStyle(.borderedProminent)
        }
    }

    // MARK: - Flow

    private func start() {
        errorMessage = nil
        status = .requesting
        pollTask?.cancel()
        Task {
            do {
                let api = makeAPI()
                let created = try await api.createDeviceLoginChallenge(subjectId: nil)
                challenge = created
                status = .waitingForApproval
                startPolling(challenge: created, api: api)
            } catch {
                errorMessage = error.localizedDescription
                status = .error
            }
        }
    }

    private func startPolling(challenge: DeviceLoginChallenge, api: PendingCrewAPI) {
        // 4 分钟超时上限 (challenge.expiresAt 在 server 端是 10 分钟,
        // 但 spec 要求 client 侧 4 分钟超时, 提前提示用户)。
        let deadline = Date().addingTimeInterval(4 * 60)
        pollTask = Task {
            while !Task.isCancelled {
                if Date() > deadline {
                    await MainActor.run {
                        errorMessage = "等待批准超时,请重新发起。"
                        status = .error
                    }
                    return
                }
                do {
                    let response = try await api.pollDeviceLoginChallenge(
                        challengeId: challenge.challengeId,
                        secret: challenge.secret
                    )
                    if response.status == "approved",
                       let token = response.deviceGrantToken {
                        await MainActor.run {
                            do {
                                try model.saveDeviceGrantToken(token)
                                // 家族凭据（pfa_*）只在 consume 这一拍随 grant
                                // 下发 —— 落进共享 keychain 组，家族 app 之后
                                // 静默 mint 免扫码（登录 SSO C1）。subjectId
                                // 缺失就不存（凭据没有默认 mint 目标没意义）。
                                if let fam = response.familyCredential,
                                   let sid = response.subjectId {
                                    model.saveFamilyCredential(
                                        token: fam,
                                        subjectId: sid,
                                        displayName: response.displayName,
                                        avatarSeed: response.avatarSeed
                                    )
                                }
                                status = .approved
                            } catch {
                                errorMessage = "保存登录凭据失败: \(error.localizedDescription)"
                                status = .error
                            }
                        }
                        return
                    }
                    if response.status == "rejected" {
                        await MainActor.run {
                            errorMessage = "登录请求被拒绝。"
                            status = .error
                        }
                        return
                    }
                    if response.status == "expired" {
                        await MainActor.run {
                            errorMessage = "二维码已过期,请重新发起。"
                            status = .error
                        }
                        return
                    }
                } catch let PendingCrewAPIError.http(httpStatus, code, message) {
                    // 410 session_expired / 409 conflict — 终态
                    if httpStatus == 410 || code == "session_expired" {
                        await MainActor.run {
                            errorMessage = message ?? "二维码已过期,请重新发起。"
                            status = .error
                        }
                        return
                    }
                    // 其它瞬时错误就继续重试,不打扰用户
                } catch {
                    // 网络抖动 — 继续 poll
                }
                try? await Task.sleep(nanoseconds: 2_000_000_000)
            }
        }
    }

    private func makeAPI() -> PendingCrewAPI {
        let url = URL(string: model.apiBaseURL) ?? URL(string: "https://api.pendingname.com")!
        return PendingCrewAPI(baseURL: url)
    }

    // MARK: - QR rendering

    private func qrImage(payload: String) -> Image? {
        let context = CIContext()
        let filter = CIFilter.qrCodeGenerator()
        filter.setValue(Data(payload.utf8), forKey: "inputMessage")
        filter.setValue("M", forKey: "inputCorrectionLevel")
        guard let output = filter.outputImage else { return nil }
        let scaled = output.transformed(by: CGAffineTransform(scaleX: 10, y: 10))
        guard let cgImage = context.createCGImage(scaled, from: scaled.extent) else { return nil }
        #if canImport(AppKit)
        let nsImage = NSImage(cgImage: cgImage, size: NSSize(width: scaled.extent.width, height: scaled.extent.height))
        return Image(nsImage: nsImage)
        #elseif canImport(UIKit)
        let uiImage = UIImage(cgImage: cgImage)
        return Image(uiImage: uiImage)
        #else
        return nil
        #endif
    }
}
