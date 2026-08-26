import Foundation
import SwiftUI

/// PendingCrew 顶层状态。
///
/// **本地为家**（接合 v2，spec 2026-06-10）：
/// - macOS 上 `backend` **恒为 `LocalBackend`** —— 本地 crew 永远在、永远显示。
/// - iOS 暂无本地后端（LocalRunner 是 macOS-only）→ `backend` 恒 nil，
///   等本地后端跨平台后统一。
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

    /// 当前生效的 backend。
    /// - macOS:恒为 `LocalBackend` —— 本地 crew 是常驻 home。
    /// - iOS:**恒 nil**(空壳)。LocalRunner 是 macOS-only,而云端那条路随 #63
    ///   第二期整层删除；等本地后端跨平台后统一成恒本地。
    var backend: PendingCrewBackend? {
        #if os(macOS)
        return localBackend
        #else
        return nil
        #endif
    }
}
