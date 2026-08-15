#if os(macOS)
import Foundation

/// 某个项目仓库当下的同步状态快照——`ProjectSyncService.scan` 的产出,给 UI
/// 渲染"这个项目现在是什么状态"(clean/dirty/领先/落后/机器上压根没这个目录)。
public struct ProjectSyncStatus: Equatable, Sendable {
    public var projectId: String
    public var branch: String?
    public var head: String?
    public var dirty: Bool
    public var ahead: Int
    public var behind: Int
    /// 本机压根没有该目录(还没 clone/初始化)。
    public var missing: Bool

    public init(
        projectId: String,
        branch: String?,
        head: String?,
        dirty: Bool,
        ahead: Int,
        behind: Int,
        missing: Bool
    ) {
        self.projectId = projectId
        self.branch = branch
        self.head = head
        self.dirty = dirty
        self.ahead = ahead
        self.behind = behind
        self.missing = missing
    }
}

/// 扫描 + 推送——把 `WorkspaceGit` 的 plumbing 组装成"像 git push 一样可验证"的
/// 同步动作(spec §4.2)。
///
/// **错误处理的两层纪律**(brief「Before You Begin」明确过,这里落到代码里):
/// - **配置级错误**(目录不存在只影响 `scan` 的 `missing` 分支;不是 git repo、
///   git 不在 PATH 等)**向上抛**,让调用方(UI)如实呈现——这些
///   不是"同步没做成",而是"环境本身有问题",吞掉只会让用户看到一个语焉不详的
///   `.failed` 回执,debug 时无从下手。
/// - **运行期错误**(`git push`/`ls-remote` 这两步——本质是网络操作,远端不可达/
///   认证失败/push 后复核不一致)**包成 `.failed(reason:)` 回执,不抛**——这是
///   同步循环的常态(网络时断时续),不该打断调用方的控制流,应该体现在回执里。
public enum ProjectSyncService {

    /// 扫一眼某个项目仓库现在的状态。
    ///
    /// - `localPath` 不存在(或存在但不是目录)→ `missing = true`,其余字段归零
    ///   (`branch`/`head` 为 nil,`dirty = false`,`ahead`/`behind` = 0)——这不是
    ///   异常,是"这台机器还没这个项目"的正常状态。
    /// - 目录存在但不是 git repo / git 命令失败 → **不吞**,原样向上抛
    ///   `WorkspaceGit.GitError`(brief 明确否决了"吞成 dirty=false/ahead=0"的
    ///   选项——那样会让 UI 把"环境坏了"误显示成"已同步")。
    /// - `fetchFirst`:是否先 `git fetch` 再读 ahead/behind(联网时机由调用方
    ///   决定,呼应 `WorkspaceGit.aheadBehind` 的"不隐式 fetch"设计)。
    /// - detached HEAD(`currentBranch` 为 nil)时没有"当前分支"可比较
    ///   ahead/behind,两者归零,不当错误处理——`scan` 只是只读快照,不像
    ///   `pushCurrentBranch` 那样必须确定一个分支才能继续。
    public static func scan(
        projectId: String, localPath: URL, fetchFirst: Bool
    ) throws -> ProjectSyncStatus {
        var isDirectory: ObjCBool = false
        let exists = FileManager.default.fileExists(atPath: localPath.path, isDirectory: &isDirectory)
        guard exists, isDirectory.boolValue else {
            return ProjectSyncStatus(
                projectId: projectId, branch: nil, head: nil,
                dirty: false, ahead: 0, behind: 0, missing: true
            )
        }

        if fetchFirst {
            try WorkspaceGit.fetch(at: localPath)
        }

        let branch = try WorkspaceGit.currentBranch(at: localPath)
        let head = try WorkspaceGit.head(at: localPath)
        let dirty = try WorkspaceGit.isDirty(at: localPath)

        var ahead = 0
        var behind = 0
        if let branch {
            let counts = try WorkspaceGit.aheadBehind(at: localPath, branch: branch)
            ahead = counts.ahead
            behind = counts.behind
        }

        return ProjectSyncStatus(
            projectId: projectId, branch: branch, head: head,
            dirty: dirty, ahead: ahead, behind: behind, missing: false
        )
    }

    /// 推当前分支,产出回执 + (成功时)供调用方写回 manifest 的 `LastSync`。
    ///
    /// 流程(brief §Interfaces 逐字对应,外加 review 裁决的第 0 步):
    /// 0. detached HEAD(`currentBranch` 为 nil)→ `.skipped("detached HEAD,
    ///    请先切回分支")`,**放在最前、任何 mutate 之前**——否则 dirty+wipCommit
    ///    路径会先 commitAll 再发现无分支可推,白造孤儿 commit。
    /// 1. dirty && !wipCommit → `.skipped("有未提交变更")`,`LastSync` 为 nil
    ///    (调用方不该写回一个没发生过的同步)。
    /// 2. dirty && wipCommit → 先 `WorkspaceGit.commitAll("wip: workspace sync
    ///    auto commit")`,让工作区收敛成 clean,再往下走。
    /// 3. (此时已不 dirty)读 `aheadBehind` 判断是否真需要 push:`ahead == 0` →
    ///    `.upToDate`,不碰网络。
    /// 4. `ahead > 0` → `git push`;push 抛出的错误(远端不可达等**运行期**
    ///    错误)在这里捕获,包成 `.failed(reason:)`,不上抛。
    /// 5. push 命令退出码为 0 之后**不能直接认定成功**——再 `lsRemoteHead`
    ///    独立复核远端 ref 是否等于本地 head(spec §4.2,`Outcome` 文档也重申了
    ///    这条纪律):相等才发 `.uploaded(remoteHead:)` + 构造 `LastSync`;
    ///    不相等,或复核这一步本身失败(同样是网络操作),都归 `.failed`。
    ///
    /// `machineId` 由调用方传入——`LastSync.machine` 该写哪台机器的标识,这层
    /// 服务不关心"当前机器是谁"这种环境探测,只负责把调用方给的值塞进结构里。
    public static func pushCurrentBranch(
        projectId: String,
        localPath: URL,
        remoteURL: String,
        wipCommit: Bool,
        machineId: String,
        now: () -> Date = Date.init
    ) throws -> (SyncReceipt, LastSync?) {
        // 一次动作一个 formatter,receipt.at 与 LastSync.pushedAt 同源同格式。
        let iso = ISO8601DateFormatter()
        func makeReceipt(_ outcome: Outcome) -> SyncReceipt {
            SyncReceipt(
                itemId: projectId, kind: .project, outcome: outcome,
                at: iso.string(from: now())
            )
        }

        // 0. **先**确认在分支上,再考虑任何 mutate。detached HEAD 是可恢复的普通
        //    状态(用户在翻历史/bisect),不是错误——回 `.skipped` 让 UI 如实展示,
        //    且**绝不能先 commitAll 再发现无分支可推**:那会白白造出一个没有分支
        //    引用的孤儿 wip commit。
        guard let branch = try WorkspaceGit.currentBranch(at: localPath) else {
            return (makeReceipt(.skipped(reason: "detached HEAD,请先切回分支")), nil)
        }

        // 1-2. dirty 分流:不允许自动 commit → skipped;允许 → 先落地成一个提交。
        if try WorkspaceGit.isDirty(at: localPath) {
            guard wipCommit else {
                return (makeReceipt(.skipped(reason: "有未提交变更")), nil)
            }
            _ = try WorkspaceGit.commitAll(at: localPath, message: "wip: workspace sync auto commit")
        }

        // 3. 已经是最新(没有领先本地提交)就不用碰网络。
        let ahead = try WorkspaceGit.aheadBehind(at: localPath, branch: branch).ahead
        guard ahead > 0 else {
            return (makeReceipt(.upToDate), nil)
        }

        let localHead = try WorkspaceGit.head(at: localPath)

        // 4. push 是网络操作——失败(远端不可达/认证失败等)包成 .failed,不上抛。
        do {
            try WorkspaceGit.push(at: localPath, branch: branch)
        } catch {
            return (makeReceipt(.failed(reason: "push 失败: \(error)")), nil)
        }

        // 5. push 退出码 0 不等于"确认送达"——独立复核远端 ref。
        let remoteHead: String?
        do {
            remoteHead = try WorkspaceGit.lsRemoteHead(remoteURL: remoteURL, branch: branch, cwd: localPath)
        } catch {
            return (makeReceipt(.failed(reason: "push 后复核远端 ref 失败: \(error)")), nil)
        }

        guard let remoteHead, remoteHead == localHead else {
            return (makeReceipt(.failed(reason: "push 后远端 ref 与本地不一致")), nil)
        }

        let lastSync = LastSync(
            machine: machineId, branch: branch, head: localHead,
            pushedAt: iso.string(from: now())
        )
        return (makeReceipt(.uploaded(remoteHead: remoteHead)), lastSync)
    }
}
#endif
