#if os(macOS)
import Foundation

/// 一次同步动作覆盖的「资源种类」——目前只有 workspace 同步的项目仓库
/// (`project`)会真正走到 `ProjectSyncService`,其余几个 case 是给后续 Task(state/
/// env/secrets 各自的同步器)预留的统一词汇表,不在本 task 里产出。
public enum SyncItemKind: String, Codable, Equatable, Sendable {
    case project
    case workspaceRepo
    case state
    case env
    case secrets
}

/// 一次同步动作的结果——**「像 git push 一样可验证」的回执**(spec §4.2)。
///
/// 设计动机:UI/日志只想知道"这次同步到底发生了什么",不想重新解释 git 的
/// exit code / stderr。`SyncReceipt` 把 `ProjectSyncService` 内部的 git 操作序列
/// 收口成一个可读、可持久化、可比较相等的值——`itemId`+`kind` 定位"同步的是
/// 什么",`outcome` 是结果,`at` 是发生时间(ISO8601 字符串,理由同
/// `LastSync.pushedAt`:diff/日志里更直观,详见 WorkspaceManifest.swift 头注释)。
public struct SyncReceipt: Equatable, Sendable {
    public var itemId: String
    public var kind: SyncItemKind
    public var outcome: Outcome
    /// ISO8601 字符串。
    public var at: String
    /// **Task 7 最小新增**:通用的补充说明,默认 `nil`——不往 `Outcome` 的 case
    /// 签名里加关联值(那会牵动所有既有 `switch`/pattern-match 调用点),而是在
    /// `SyncReceipt` 这一层加一个平行字段。目前唯一的产出者是
    /// `SyncEngine.executeDown`:pull/clone 之后本机 head 超过 `last_sync.head`
    /// 记录的旧点(remote 比 manifest 新,不是错误)时,`outcome` 仍然如实是
    /// `.pulled(newHead:)`,但"为什么 newHead 跟 manifest 记的不一样"这句解释
    /// 放在这里,而不是编造进 `Outcome` 本该只表达"发生了什么"的关联值里。
    public var detail: String?

    public init(itemId: String, kind: SyncItemKind, outcome: Outcome, at: String, detail: String? = nil) {
        self.itemId = itemId
        self.kind = kind
        self.outcome = outcome
        self.at = at
        self.detail = detail
    }
}

/// 一次同步动作的四(五)种可能结果。
///
/// **`.uploaded` 的发放纪律(spec §4.2)**——这是这个类型里唯一"容易做错"的地方:
/// `.uploaded(remoteHead:)` **只有在 `git push` 之后,再用 `lsRemoteHead` 独立
/// 复核「远端 ref 确实等于本地 head」通过后才能构造**。不能因为 `git push`
/// 命令本身退出码是 0 就认为"传上去了"——push 命令成功只代表"客户端这边没报错",
/// 不代表远端真的收敛到了预期状态(理论上存在诸如 pre-receive hook 静默改写/
/// 竞态被别的 push 覆盖之类的边界情况)。多花一次 `ls-remote` 网络往返,换来
/// "回执等于事实"这个不可动摇的保证,这正是"像 git push 一样可验证"里
/// "可验证"三个字的落地。
///
/// - `.upToDate`:压根不需要 push(没有本地领先的提交,也不 dirty)。
/// - `.skipped(reason:)`:有意不做——dirty 且调用方不允许自动 commit,或
///   detached HEAD(可恢复的普通状态,用户切回分支即解,不算错误)。
/// - `.failed(reason:)`:尝试过(commit/push/复核)但没达成预期状态——**运行期**
///   错误(远端不可达、push 后复核不一致等)包成这个 case 而不是往上抛异常,
///   见 `ProjectSyncService.pushCurrentBranch` 的文档:只有「配置级」错误(目录
///   不存在、不是 git repo 等)才用 Swift `throws` 向上抛,运行期的"这次没做成"
///   是同步循环的常态,应该体现在回执里让 UI 如实展示,不该打断调用方的控制流。
/// - `.pulled(newHead:)`:**Task 5 预留** —— 拉取/合并远端新提交后的回执,本
///   task 只声明这个 case,不产出也不测试(Task 5/7 覆盖)。
public enum Outcome: Equatable, Sendable {
    case uploaded(remoteHead: String)
    case upToDate
    case skipped(reason: String)
    case failed(reason: String)
    case pulled(newHead: String)
}
#endif
