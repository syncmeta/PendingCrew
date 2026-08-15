#if os(macOS)
import Foundation

/// workspace **仓库自身**(`workspace.toml` + `projects/*.toml` + …,不是里面声明的
/// 各个项目仓库)的生命周期与同步入口 —— `ensure` 负责"这台机器上到底有没有这个
/// workspace 仓库、没有的话怎么落地",`syncUp`/`syncDown` 负责"把本机的 workspace
/// 仓库往远端推 / 从远端拉"。
///
/// **与 `ProjectSyncService` 的分工边界**:`ProjectSyncService` 管的是 workspace 里
/// *声明的项目仓库*(用户代码 checkout),这一层管 workspace 仓库*本体*——一个纯
/// 配置/manifest 仓库,没有需要保护的长期分叉历史。这也是为什么这一层的 pull 策略
/// 固定用 `pullRebase`(见 `WorkspaceGit.pullRebase` 文档)而不是 `pullFastForward`:
/// 多机同时改 manifest 是常态,rebase 冲突概率低(大多数改动落在不同的
/// `projects/<id>.toml` 文件里),线性历史也更方便审计"谁什么时候改了什么"。
///
/// **`syncUp`/`syncDown` 为什么不 `throws`,而 `ensure` 要 `throws`**(brief 明确的
/// 纪律,呼应 `ProjectSyncService` 的"配置级 vs 运行期"两层错误处理):
/// - `ensure` 处理的是"这台机器第一次接触这个 workspace 仓库"——目录状态、
///   `remoteURL` 是否传对,这些是调用方(UI/onboarding 流程)必须知道且能补救的
///   配置错误,原样 `throws` 上抛让调用方如实处理(比如提示用户重新填 remote
///   URL),不该吞。
/// - `syncUp`/`syncDown` 是同步循环里会反复调用的常规动作——网络时断时续、
///   远端偶尔跟不上,都是运行期常态,不该打断调用方的控制流。任何错误(包括
///   `WorkspaceGit` 抛出的配置级错误,比如目录途中被删掉)都收进
///   `Outcome.failed(reason:)` 里如实展示,不上抛。
public enum WorkspaceRepoService {

    // MARK: - ensure

    /// 保证 `root` 是一个可用的 workspace 仓库,返回其 `WorkspaceRepoLayout`。
    ///
    /// 三分支(brief 逐字对应):
    /// 1. **`root` 已经是个 workspace**(`workspace.toml` 存在)→ 直接返回,
    ///    不碰 git、不被 `remoteURL`/`name` 参数覆盖——已经存在的东西不该被入参
    ///    悄悄改写。这里刻意先判断"目录存在"再判断"是不是 workspace":如果
    ///    存在但不是 workspace(比如误指了个普通目录),`loadWorkspace()`
    ///    原样抛 `WorkspaceRepoLayout.LayoutError.notAWorkspace`,不静默降级去
    ///    scaffold——scaffold 一个已有内容的目录是危险操作,不该由这层自作
    ///    主张。
    /// 2. **`root` 不存在 且 `remoteURL` 非空** → `clone`,再分两种情况:
    ///    - **clone 下来是空仓库**(还没有任何提交——远端只是配好了一个空 bare
    ///      仓库,这是"第一台机器,但 remote 已经提前建好"的核心场景,不是
    ///      "clone 到了错误的仓库")→ 直接在这个已经 clone 好(`origin` 已由
    ///      `clone` 配好,**不需要**再 `initRepo`)的目录里 `scaffold` + 首次
    ///      `commitAll`,行为等价于分支 3,只是 remote 已经就位。
    ///      判空的方法:`WorkspaceGit.head(at:)` 在 unborn branch(没有任何
    ///      提交)上会因为 `git rev-parse HEAD` 找不到引用而抛
    ///      `.commandFailed`——用这个来判断"是空仓库"而不是自己重新实现一遍
    ///      "有没有提交"的判定。
    ///    - **clone 下来非空,但校验不是 workspace**(`workspace.toml` 缺失)
    ///      → 这才是真正的"remote 填错了"场景,`notAWorkspace` 原样上抛——
    ///      `clone` 本身已经落地(目录/内容都在),调用方能看到目录已存在以便
    ///      自行处理(不在这里自动删除清理,那是更危险的自作主张)。
    /// 3. **`root` 不存在 且 `remoteURL` 为 nil** → 本机是"第一台机器",从零
    ///    建,且这台机器压根没打算(或还没决定)配 remote:`scaffold`(建骨架 +
    ///    `workspace.toml`)→ `initRepo` → 首次 `commitAll`,让新建的 workspace
    ///    仓库一开始就有一个干净的初始提交。
    public static func ensure(
        at root: URL, remoteURL: String?, name: String
    ) throws -> WorkspaceRepoLayout {
        var isDirectory: ObjCBool = false
        let exists = FileManager.default.fileExists(atPath: root.path, isDirectory: &isDirectory)

        if exists && isDirectory.boolValue {
            let layout = WorkspaceRepoLayout(root: root)
            _ = try layout.loadWorkspace() // 不是 workspace 就原样抛 notAWorkspace
            return layout
        }

        if let remoteURL {
            try WorkspaceGit.clone(remoteURL: remoteURL, to: root)

            let clonedIsEmpty: Bool
            do {
                _ = try WorkspaceGit.head(at: root)
                clonedIsEmpty = false
            } catch WorkspaceGit.GitError.commandFailed {
                // unborn branch(clone 下来的 remote 还没有任何提交)—— `git
                // rev-parse HEAD` 找不到引用会走这条路径,这是判空的信号。
                clonedIsEmpty = true
            }

            if clonedIsEmpty {
                // remote 已经由 clone 配好(origin 指向它),这里只需要把骨架
                // 铺进这个已经是 git repo 的目录、首次提交——不能再调用
                // `initRepo`(会重新 `git init` 一个已经有 `.git`/`origin` 的
                // 目录,虽然幂等但语义上没必要,也没必要重设 remote)。
                let layout = try WorkspaceRepoLayout.scaffold(at: root, name: name)
                _ = try WorkspaceGit.commitAll(at: root, message: "chore: init workspace repo")
                return layout
            }

            let layout = WorkspaceRepoLayout(root: root)
            _ = try layout.loadWorkspace() // 非空但不是 workspace → 原样抛 notAWorkspace
            return layout
        }

        let layout = try WorkspaceRepoLayout.scaffold(at: root, name: name)
        try WorkspaceGit.initRepo(at: root)
        _ = try WorkspaceGit.commitAll(at: root, message: "chore: init workspace repo")
        return layout
    }

    // MARK: - syncUp

    /// 把 `layout` 对应的 workspace 仓库推到远端,产出"像 git push 一样可验证"
    /// 的回执(spec §4.2,同 `ProjectSyncService.pushCurrentBranch` 的复核纪律)。
    ///
    /// 流程(brief 逐字对应):
    /// 1. `commitAll(message)` —— workspace 仓库自身没有"dirty 就 skip"这层
    ///    顾虑(不像项目仓库那样可能是用户手写的代码,还没准备好提交):manifest
    ///    变更直接落地成一个提交,调用方无需关心 wipCommit 开关。
    /// 2. 查 `WorkspaceGit.remoteURL` —— 没配 → `.skipped("未配置 remote")`,
    ///    这是新建 workspace 仓库、还没决定同步到哪的正常状态,不是错误。
    /// 3. detached HEAD(没有当前分支)→ `.failed("detached HEAD,无法推送")`——
    ///    workspace 仓库理论上不该跑到 detached 状态,这里如实报错而不是假装
    ///    "跳过"(不像 `ProjectSyncService` 那样把 detached 当可恢复的用户操作
    ///    ——那是给"用户在翻历史"的场景;workspace 仓库不该有这种交互)。
    /// 4. `fetch` 刷新远端可见状态,再**显式**查远端是否已经有这条分支
    ///    (`lsRemoteHead(...) != nil`——直接问,不靠"`aheadBehind` 抛不抛错"
    ///    这种隐式信号去猜,那样会把"远端没有这条分支"和"`aheadBehind` 本身
    ///    真的读失败了"混在一起,后者理应走 `.failed` 而不是被静默吞掉误判
    ///    成"这是第一次推"):
    ///    - 远端已经有这条分支 → 读 `aheadBehind`(这里的任何错误都是真错误,
    ///      直接 `.failed`,不再 `try?` 吞)。`behind > 0` → 先 `pullRebase`
    ///      兜 non-fast-forward(对方这段时间可能也推过),再重新读一次
    ///      `aheadBehind`。`ahead == 0`(commitAll 没新提交、且 pullRebase 后
    ///      本地也没有领先提交)→ `.upToDate`,不碰 push——覆盖 brief 里
    ///      "commitAll false 且远端本就一致 → upToDate"的场景。
    ///    - 远端压根没这条分支(本机第一次把这个 workspace 仓库推上去)→
    ///      没有远端历史可比、也没什么好 rebase 的,直接往下走 push
    ///      (`commitAll`/`ensure` 已经保证本地至少有一个提交,不存在
    ///      "无事可推"的空转风险)。
    /// 5. `push`;push 失败(远端不可达等**运行期**错误)包成 `.failed`,不
    ///    上抛。
    /// 6. push 退出码 0 不等于"确认送达"—— 独立 `lsRemoteHead` 复核远端 ref
    ///    是否等于本地 head:相等才发 `.uploaded(remoteHead:)`,不相等(或复核
    ///    本身失败)都归 `.failed`——**这条纪律不可绕过**(`SyncReceipt.swift`
    ///    头注释里反复强调的"回执等于事实")。
    ///
    /// 任何一步抛出的错误(包括前几步里配置级的 git 错误,比如目录途中被删)
    /// 都在最外层兜底捕获,包成 `.failed(reason:)`——`syncUp` 本身不 `throws`。
    public static func syncUp(
        layout: WorkspaceRepoLayout, message: String, now: () -> Date = Date.init
    ) -> SyncReceipt {
        let iso = ISO8601DateFormatter()
        func receipt(_ outcome: Outcome) -> SyncReceipt {
            SyncReceipt(itemId: "workspace", kind: .workspaceRepo, outcome: outcome, at: iso.string(from: now()))
        }
        let root = layout.root

        do {
            _ = try WorkspaceGit.commitAll(at: root, message: message)

            guard let remoteURL = try WorkspaceGit.remoteURL(at: root) else {
                return receipt(.skipped(reason: "未配置 remote"))
            }
            guard let branch = try WorkspaceGit.currentBranch(at: root) else {
                return receipt(.failed(reason: "detached HEAD,无法推送"))
            }

            try WorkspaceGit.fetch(at: root)

            // 显式问远端有没有这条分支,不靠 aheadBehind 抛不抛错去猜(见上方
            // 文档)。
            let remoteHasBranch: Bool
            do {
                remoteHasBranch = try WorkspaceGit.lsRemoteHead(remoteURL: remoteURL, branch: branch, cwd: root) != nil
            } catch {
                return receipt(.failed(reason: "查询远端分支失败: \(error)"))
            }

            if remoteHasBranch {
                let counts: (ahead: Int, behind: Int)
                do {
                    counts = try WorkspaceGit.aheadBehind(at: root, branch: branch)
                } catch {
                    return receipt(.failed(reason: "读取 ahead/behind 失败: \(error)"))
                }

                if counts.behind > 0 {
                    do {
                        try WorkspaceGit.pullRebase(at: root)
                    } catch {
                        return receipt(.failed(reason: "push 前 pullRebase 失败: \(error)"))
                    }
                }

                let refreshed: (ahead: Int, behind: Int)
                do {
                    refreshed = try WorkspaceGit.aheadBehind(at: root, branch: branch)
                } catch {
                    return receipt(.failed(reason: "读取 ahead/behind 失败: \(error)"))
                }
                guard refreshed.ahead > 0 else {
                    return receipt(.upToDate)
                }
            }
            // else:远端还没有这条分支(本机第一次推)——没有远端历史可比,
            // 直接往下走 push。

            let localHead = try WorkspaceGit.head(at: root)

            do {
                try WorkspaceGit.push(at: root, branch: branch)
            } catch {
                return receipt(.failed(reason: "push 失败: \(error)"))
            }

            let remoteHead: String?
            do {
                remoteHead = try WorkspaceGit.lsRemoteHead(remoteURL: remoteURL, branch: branch, cwd: root)
            } catch {
                return receipt(.failed(reason: "push 后复核远端 ref 失败: \(error)"))
            }

            guard let remoteHead, remoteHead == localHead else {
                return receipt(.failed(reason: "push 后远端 ref 与本地不一致"))
            }
            return receipt(.uploaded(remoteHead: remoteHead))
        } catch {
            return receipt(.failed(reason: "\(error)"))
        }
    }

    // MARK: - syncDown

    /// 从远端拉取 `layout` 对应 workspace 仓库的新提交(`fetch` + `pullRebase`)。
    ///
    /// - 拉之前记下本地 head,拉之后再比一次:没变化 → `.upToDate`(没有增量,
    ///   不需要通知调用方"发生了什么")。
    /// - 变了 → `.pulled(newHead:)`(`Outcome` 里 Task 5 新增的 case——`.uploaded`
    ///   语义是"我推上去了",不适合描述"我拉下来了"这个方向相反的结果)。
    /// - 任何失败(fetch 网络错误、pullRebase 冲突等)包成 `.failed(reason:)`,
    ///   不上抛——同 `syncUp`,这是同步循环的常规调用,运行期错误应该体现在
    ///   回执里。
    public static func syncDown(
        layout: WorkspaceRepoLayout, now: () -> Date = Date.init
    ) -> SyncReceipt {
        let iso = ISO8601DateFormatter()
        func receipt(_ outcome: Outcome) -> SyncReceipt {
            SyncReceipt(itemId: "workspace", kind: .workspaceRepo, outcome: outcome, at: iso.string(from: now()))
        }
        let root = layout.root

        do {
            let beforeHead = try WorkspaceGit.head(at: root)
            try WorkspaceGit.fetch(at: root)
            try WorkspaceGit.pullRebase(at: root)
            let afterHead = try WorkspaceGit.head(at: root)

            guard afterHead != beforeHead else {
                return receipt(.upToDate)
            }
            return receipt(.pulled(newHead: afterHead))
        } catch {
            return receipt(.failed(reason: "\(error)"))
        }
    }
}
#endif
