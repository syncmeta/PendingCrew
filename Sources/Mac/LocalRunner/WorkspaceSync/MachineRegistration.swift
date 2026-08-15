#if os(macOS)
import Foundation

/// 「本机在这个 workspace 仓库里登记一下自己」——纯函数,不碰 git/网络,只管写
/// `machines/<machineId>.toml`。独立于 `WorkspaceSetupSheet`/`WorkspaceSyncStore`
/// 抽出来,方便无 UI 单测(Task 9 brief)。
///
/// **幂等纪律**:这台机器可能已经注册过(比如之前跑过一次 setup、或者
/// `machines/<id>.toml` 是别的机器同步下来时就已经带了 `localPathOverrides`
/// ——虽然目前 override 只由用户手改,但字段本身在 manifest 里已经存在,不能
/// 假设永远是空)。重复调用 `register` 只更新 `displayName`,已有的
/// `localPathOverrides` 原样保留,不被这次「重新登记」覆盖成空——那会丢用户
/// 手动配置的 per-machine 路径覆盖。
public enum MachineRegistration {
    /// 登记 `machineId` 到 `layout` 对应的 workspace 仓库。
    ///
    /// - 该机器还没注册过 → 新建 `MachineManifest(displayName:)`,
    ///   `localPathOverrides` 为空。
    /// - 该机器已注册过 → 只替换 `displayName`,原样保留已有的
    ///   `agePublicKeyFingerprint`/`localPathOverrides`。
    public static func register(
        layout: WorkspaceRepoLayout, machineId: String, displayName: String
    ) throws {
        if let existing = try layout.loadMachine(id: machineId) {
            var updated = existing
            updated.displayName = displayName
            try layout.writeMachine(id: machineId, updated)
        } else {
            try layout.writeMachine(id: machineId, MachineManifest(displayName: displayName))
        }
    }
}
#endif
