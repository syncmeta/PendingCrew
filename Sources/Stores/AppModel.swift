import Foundation
import SwiftUI

/// PendingCrew 顶层状态。
///
/// Scaffold 版只保留 auth + api base url。Phase 1+ 按 spec v2 加上：
/// - subject 列表 / 当前 subject
/// - crew 树（DAG）
/// - runner hosts / machines
/// - 等等
///
/// **不要**回到老 PendingCrew 的 10+ @Published 字典爆炸路径
/// （那是 codex 留下的待重构债务）。新增状态应归并成结构化 model
/// （如 `CrewDetail` 容器），见 spec v2 §6.2 / roadmap §11。
///
/// **接合 v2(spec 2026-06-10):双轨 backend 作废,本地为家**:
/// - macOS 上 `backend` **恒为 `LocalBackend`** —— 本地 crew 永远在、永远显示。
/// - 登录(`credential.kind == .deviceGrant`)只是账号级**能力叠加**
///   (遥控 / 未来信箱 + 邀请),不再切换 crew 数据来源。能力判定用
///   `isAuthenticated`,不要再用 `mode` 路由数据。
/// - iOS 暂无本地后端(LocalRunner 是 macOS-only),仍走 EdgeBackend,
///   等本地后端跨平台后统一。
@MainActor
final class AppModel: ObservableObject {
    @Published var apiBaseURL: String
    @Published private(set) var credential: PendingCrewAuthCredential?

    /// 登录这台机的 PendingCrew 用户(auth.users.id)。best-effort:在
    /// subject 解析出来(`CrewStore.refreshSubjects` → `/v1/me/subject` 回
    /// `user_id`)之前为 nil。群聊 `CrewChatView` 用它把
    /// `CrewWhiteboardEntry.senderUserId` 等于本值的消息识别成"自己的气泡"
    /// (右对齐、无头像);多 human crew 里其他人的消息因此不会被误判成自己。
    @Published private(set) var currentUserId: String?

    private let defaults: UserDefaults

    /// 两条 backend 实例,启动时一次性构造。
    /// 不在 each-call 时 new (避免 EdgeBackend 内每次重新解析 credential
    /// 但 instance 不复用的浪费;LocalBackend.store 是 shared singleton)。
    private lazy var edgeBackend: EdgeBackend = EdgeBackend(appModel: self)
    private lazy var localBackend: LocalBackend = LocalBackend(store: .shared, whiteboard: .shared)

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        self.apiBaseURL = defaults.string(forKey: DefaultsKey.apiBaseURL) ?? "https://api.pendingname.com"
        // 用局部变量算，避免 init 里在所有 stored 属性赋值前读 self.x（definite-init）。
        let cred = PendingCrewAuthCredential.resolveDeviceGrant(
            KeychainStore.get(account: KeychainAccount.deviceGrant)
        )
        self.credential = cred
    }

    /// 当前生效的 backend(接合 v2:**不再按 mode 路由**)。
    /// - macOS:恒为 `LocalBackend` —— 本地 crew 是常驻 home,登录只叠加能力。
    /// - iOS:暂无本地后端(LocalRunner 是 macOS-only),登录态走 EdgeBackend,
    ///   未登录返回 nil。#63 删掉登录入口后 iOS 上这里恒 nil(空壳),
    ///   等本地后端跨平台后统一成恒本地。
    var backend: PendingCrewBackend? {
        #if os(macOS)
        return localBackend
        #else
        return credential != nil ? edgeBackend : nil
        #endif
    }

    /// **#63 之后已无消费者** —— `RootView` 不再按它分叉,登录入口整块删了。
    /// 跟凭据层一起留到第二期随 `Sources/Remote/` 一刀端掉,见 docs/tech-debt.md。
    /// **Mac 恒 true**:本地为家(本地 backend 常驻)。iOS 恒 false(无本机后端)。
    var isConfigured: Bool {
        #if os(macOS)
        return true
        #else
        return credential != nil
        #endif
    }

    /// `isAuthenticated` 仅指"登录态(device grant)"。接合 v2 后这是唯一的能力门:
    /// 登录 = 账号级能力叠加(遥控/未来信箱),不再有 `mode` 路由。
    var isAuthenticated: Bool { credential != nil }

    func saveAPIBaseURL(_ url: String) {
        let trimmed = url.trimmingCharacters(in: .whitespacesAndNewlines)
        apiBaseURL = trimmed
        defaults.set(trimmed, forKey: DefaultsKey.apiBaseURL)
    }

    func saveDeviceGrantToken(_ token: String) throws {
        try KeychainStore.set(token, account: KeychainAccount.deviceGrant)
        credential = PendingCrewAuthCredential.resolveDeviceGrant(token)
    }

    func clearAuth() {
        // 注意：这里**不**清 FamilyCredentialStore —— 家族凭据是账号级、跨 app
        // 共享的（PendingBot / 其它家族成员都靠它静默 mint），签出本 app 的
        // device grant 不该连坐。真要吊销家族凭据走 PendingBot 端的管理入口。
        KeychainStore.delete(account: KeychainAccount.deviceGrant)
        credential = nil
        currentUserId = nil
    }

    // MARK: - 家族 SSO（登录 SSO B4）

    /// QR 登录 consume 时随 grant 下发的家族凭据（`pfa_*`）落到共享 keychain
    /// 组，供家族 app（含本 app 重装后）静默 mint。
    /// **#63 之后无调用方** —— 唯一的调用点（扫码登录页）已删。
    func saveFamilyCredential(token: String, subjectId: String, displayName: String? = nil, avatarSeed: String? = nil) {
        FamilyCredentialStore.set(FamilyCredential(
            token: token,
            subjectId: subjectId,
            displayName: displayName,
            avatarSeed: avatarSeed
        ))
    }

    /// 共享 keychain 组里是否有家族凭据。
    /// **#63 之后无调用方** —— 侧栏那个登录入口已删。
    var familySSOAvailable: Bool { FamilyCredentialStore.get() != nil }

    /// 家族 SSO 登录：用共享 keychain 组里的家族凭据调 `POST /v1/device-grant/mint`
    /// 换本 app 自己的 scoped device grant，免扫码。**不在启动时自动跑** ——
    /// 登录必须是用户显式动作。**#63 之后无调用方** —— 侧栏那个入口已删。
    @discardableResult
    func tryFamilySSO() async -> Bool {
        guard credential == nil else { return true }
        guard let cred = FamilyCredentialStore.get() else { return false }
        guard let url = URL(string: apiBaseURL) else { return false }
        // mint 不需要本 app 的 grant —— 鉴权是家族凭据本身作 bearer，
        // 所以跟 device-login 一样用无 token 的 client。
        let api = PendingCrewAPI(baseURL: url)
        do {
            let resp = try await api.mintGrant(
                familyCredential: cred.token,
                subjectId: cred.subjectId,
                grantKind: "pendingcrew_control",
                scopes: ["subject:read", "crew:read", "crew:write", "runner:read", "runner:write"]
            )
            if let token = resp.deviceGrantToken {
                try saveDeviceGrantToken(token)   // isAuthenticated 翻 true
                return true
            }
            return false
        } catch {
            print("[family-sso] login failed: \(error)")
            return false
        }
    }

    /// 由 `CrewStore.refreshSubjects` 在解析到 subject 时回填(best-effort)。
    /// 只在拿到非 nil 值时更新,避免空 userId 覆盖已解析出的真实 user id。
    func setCurrentUserIdIfResolved(_ userId: String?) {
        guard let userId, !userId.isEmpty else { return }
        currentUserId = userId
    }

    // MARK: - T4.5 crew-session server lifecycle

    /// 本 app session 缓存的 runner-host id —— 第一个 logged crew session 时
    /// 懒注册,之后复用;重启后重新注册。
    private var cachedRunnerHostId: String?

    /// auth-gated 图片加载所需的 (baseURL, device-grant token)。群聊附件
    /// `CrewRemoteImage` 用它给 `/v1/uploads/<id>` 带上 `Authorization: Bearer`。
    /// 非登录态(无 device grant)或 baseURL 无效时返回 nil —— 调用方降级到
    /// 占位图,不崩。
    var imageAuth: (baseURL: URL, token: String)? {
        guard let credential, credential.kind == .deviceGrant,
              let url = URL(string: apiBaseURL) else { return nil }
        return (url, credential.token)
    }

    /// 用当前 device-grant 凭据构造一个 `PendingCrewAPI`。非登录态抛错。
    func loggedAPIClient() throws -> PendingCrewAPI {
        guard let credential, credential.kind == .deviceGrant else {
            throw PendingCrewBackendError.notAuthenticated
        }
        guard let url = URL(string: apiBaseURL) else {
            throw PendingCrewBackendError.invalidConfig("API base URL 无效")
        }
        return PendingCrewAPI(baseURL: url, bearerToken: credential.token)
    }

    /// 把这台机注册成 runner host(每个 app session 一次,缓存 id 复用)。
    func ensureRunnerHost(
        api: PendingCrewAPI,
        subjectId: String,
        allowedRunnerKinds: [String]
    ) async throws -> String {
        if let id = cachedRunnerHostId { return id }
        let id = try await api.registerRunnerHost(
            responsibleSubjectId: subjectId,
            displayName: ProcessInfo.processInfo.hostName,
            allowedRunnerKinds: allowedRunnerKinds
        )
        cachedRunnerHostId = id
        return id
    }
}

private enum DefaultsKey {
    static let apiBaseURL = "PendingCrew.apiBaseURL"
}

enum KeychainAccount {
    static let deviceGrant = "device-grant"
}
