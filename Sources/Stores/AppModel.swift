import Foundation
import SwiftUI

/// PendingCrew 顶层状态。
///
/// **本地为家**（接合 v2，spec 2026-06-10）：
/// - macOS 上 `backend` **恒为 `LocalBackend`** —— 本地 crew 永远在、永远显示。
/// - iOS 没有本地账本；有持久配对时使用 `RemotePendingCrewBackend`，否则显式报未配置。
///
/// #63 第二期之前这里还挂着整片凭据层（`credential` / `isAuthenticated` /
/// `imageAuth` / `loggedAPIClient()` / `ensureRunnerHost` / `apiBaseURL` /
/// `currentUserId` / 家族 SSO）。跨端遥控整层删除后一个都不剩，这个类退化成
/// 「谁是当前 backend」这一个问题的答案。
///
/// **不要**回到老 PendingCrew 的 10+ @Published 字典爆炸路径
/// （那是 codex 留下的待重构债务）。新增状态应归并成结构化 model
/// （如 `CrewDetail` 容器），见 spec v2 §6.2 / roadmap §11。
@MainActor
final class AppModel: ObservableObject {
    /// 启动时一次性构造,不在 each-call 时 new(LocalBackend.store 是 shared singleton)。
    private lazy var localBackend: LocalBackend = LocalBackend(store: .shared, whiteboard: .shared)
    #if os(iOS)
    private var remoteBackend: RemotePendingCrewBackend?
    @Published private(set) var backendConfigurationError: String?

    init() { reloadRemoteBackend() }

    func reloadRemoteBackend() {
        remoteBackend?.disconnect()
        do {
            guard let configuration = try RemoteBackendConfiguration.production() else {
                remoteBackend = nil
                backendConfigurationError = "尚未配对远端 Mac。请导入 Mac 生成的一次性邀请。"
                return
            }
            remoteBackend = RemotePendingCrewBackend(configuration: configuration)
            backendConfigurationError = nil
        } catch {
            remoteBackend = nil
            backendConfigurationError = "远端配置不可用：\(error.localizedDescription)"
        }
    }
    #endif

    /// 当前生效的 backend。
    /// - macOS:恒为 `LocalBackend` —— 本地 crew 是常驻 home。
    /// - iOS:从 Keychain 身份 + trust/backend 持久记录构造安全远端 backend。
    var backend: PendingCrewBackend? {
        #if os(macOS)
        return localBackend
        #else
        return remoteBackend
        #endif
    }
}
