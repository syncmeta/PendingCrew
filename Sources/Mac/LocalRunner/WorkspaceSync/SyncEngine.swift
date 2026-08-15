#if os(macOS)
import Foundation

/// 「上行同步」计划里的一项——UI 逐条呈现给用户/agent 确认(spec §4.1 步骤 2:
/// 「列表呈现：逐项列出『未同步项』，用户确认」)。
public struct SyncPlanItem: Equatable, Identifiable, Sendable {
    public var id: String
    public var kind: SyncItemKind
    public var title: String
    public var detail: String
    /// 这一项在 `executeUp` 里会不会真的被执行——`false` 的项只是「如实告知」
    /// (本机未 checkout / 扫描失败 / 本机有未处理的冲突),不参与执行。
    public var actionable: Bool

    public init(id: String, kind: SyncItemKind, title: String, detail: String, actionable: Bool) {
        self.id = id
        self.kind = kind
        self.title = title
        self.detail = detail
        self.actionable = actionable
    }
}

/// 上行同步的编排层——把 Task 1-5 的积木(`WorkspaceRepoLayout` 读写 manifest、
/// `ProjectSyncService` 扫描/推送单个项目仓库、`WorkspaceRepoService.syncUp` 推
/// workspace 仓库自身)串成 spec §4.1「一次上行」的完整流程:
///
/// ```
/// planUp    —— 扫描:各项目仓库状态 + workspace 仓库自身 dirty,产出待确认清单
/// executeUp —— 执行:逐项目 push → 回写 manifest → 收尾推 workspace 仓库
/// ```
///
/// **`SyncEngine` 是纯值类型(`layout` + `machineId` 两个 stored property)**——
/// 不持有可变状态,每次调用都是"这一刻扫一遍/推一遍",符合 spec §4.1「显式触发」
/// 的设计(不是常驻轮询的后台服务)。
///
/// **错误处理的分界**(brief 明确、呼应 `ProjectSyncService`/`WorkspaceRepoService`
/// 已经立下的两层纪律):
/// - `throws` 只用于**配置级**错误——`layout.loadProjects()` 读不了 `projects/`
///   目录、`WorkspaceGit.isDirty` 发现 workspace 仓库自己都不是个合法 git repo
///   这类"环境本身有问题"的情况,原样向上抛,让调用方(UI)如实呈现,不该吞。
/// - 单个项目在 `planUp`/`executeUp` 里的**运行期**问题(某个项目仓库的 git
///   状态读不出来、push 失败、push 后 ls-remote 复核不一致)**永远不中断其余
///   项目**——`planUp` 里包成 `actionable=false` + `detail` 说明原因;
///   `executeUp` 里包成 `.failed(reason:)` receipt,调用方数 ✓/✗ 自己判断整体
///   结果,绝不因为一个项目倒霉就让其余项目也搭进去。
public struct SyncEngine: Sendable {
    public let layout: WorkspaceRepoLayout
    public let machineId: String

    public init(layout: WorkspaceRepoLayout, machineId: String) {
        self.layout = layout
        self.machineId = machineId
    }

    // MARK: - planUp

    /// 扫一遍「现在有什么需要上行」:所有 projects + workspace 仓库自身。
    ///
    /// - 干净的项目(不 dirty 且 ahead == 0)**不进** plan——列表只呈现"需要
    ///   用户/agent 关注"的项,常态下大多数项目都是干净的,不该逐条噪音式列出。
    /// - `missing`(本机压根没 checkout 这个项目)→ 进 plan 但
    ///   `actionable = false`,`detail = "本机未 checkout"`——如实告知,不静默
    ///   跳过(用户可能以为"没出现在列表里 = 已同步",而实际是"这台机器根本
    ///   没有这份代码")。
    /// - `scan` 本身抛错(目录存在但不是 git repo、git 不在 PATH 等**配置级**
    ///   问题)→ 同样进 plan 但 `actionable = false`,`detail` 带错误描述——
    ///   这是「绝不虚报」纪律的另一面:配置烂了要让用户看见,不能因为一个项目
    ///   扫描失败就让整个 `planUp` 抛出、连其余干净项目的状态都看不到。
    /// - **workspace 仓库项永远出现**(不管 dirty 与否)——`executeUp` 收尾
    ///   总会跑一次 `WorkspaceRepoService.syncUp`(manifest 变更、以及项目
    ///   push 成功后写回的 `last_sync` 都需要带上去),plan 里如实呈现这一项
    ///   即将发生,而不是让用户以为"没在清单里 = 不会触碰 workspace 仓库"。
    public func planUp(fetchFirst: Bool) throws -> [SyncPlanItem] {
        var items: [SyncPlanItem] = []
        let projects = try layout.loadProjects()

        for (projectId, project) in projects.sorted(by: { $0.key < $1.key }) {
            do {
                let localPath = try layout.effectiveLocalPath(
                    projectId: projectId, project: project, machineId: machineId)
                let status = try ProjectSyncService.scan(
                    projectId: projectId, localPath: localPath, fetchFirst: fetchFirst)

                if status.missing {
                    items.append(SyncPlanItem(
                        id: projectId, kind: .project, title: project.name,
                        detail: "本机未 checkout", actionable: false
                    ))
                    continue
                }

                guard status.dirty || status.ahead > 0 else {
                    continue // 干净,不进 plan。
                }

                items.append(SyncPlanItem(
                    id: projectId, kind: .project, title: project.name,
                    detail: planDetail(dirty: status.dirty, ahead: status.ahead),
                    actionable: true
                ))
            } catch {
                items.append(SyncPlanItem(
                    id: projectId, kind: .project, title: project.name,
                    detail: "扫描失败: \(error)", actionable: false
                ))
            }
        }

        let workspaceDirty = try WorkspaceGit.isDirty(at: layout.root)
        items.append(SyncPlanItem(
            id: "workspace", kind: .workspaceRepo, title: "Workspace 仓库",
            detail: workspaceDirty
                ? "workspace 仓库有未提交的 manifest 变更"
                : "workspace 仓库本地无未提交变更(仍会随收尾同步一次)",
            actionable: true
        ))
        return items
    }

    private func planDetail(dirty: Bool, ahead: Int) -> String {
        switch (dirty, ahead > 0) {
        case (true, true):
            return "有未提交变更,且领先远端 \(ahead) 个提交"
        case (true, false):
            return "有未提交变更"
        case (false, true):
            return "领先远端 \(ahead) 个提交"
        case (false, false):
            return "" // 不会走到这里(guard 已经过滤掉干净项)。
        }
    }

    // MARK: - executeUp

    /// 执行一次上行:只处理 `items` 里 `actionable == true` 的项。
    ///
    /// 顺序(brief 逐字对应):
    /// 1. 逐个 actionable 的 `.project` 项调 `ProjectSyncService.pushCurrentBranch`
    ///    ——每项产出的 receipt **立即**通过 `progress` 回调,调用方(UI)可以
    ///    实时刷新每一项的状态,不用等全部跑完;**一项失败(或运行期抛错)不
    ///    中断其余项目**——`pushCurrentBranch` 本身对网络类错误已经收口成
    ///    `.failed` receipt 不抛,但这里仍然用 `do/catch` 兜一层:防止运行
    ///    途中环境发生变化(比如目录在 `planUp` 之后、`executeUp` 之前被删掉)
    ///    导致抛出配置级错误,那种情况也应该体现成这一项的 `.failed` receipt,
    ///    而不是让整个 `executeUp` 提前退出、让其余项目连跑都没跑到。
    /// 2. 成功上传(`.uploaded`)的项目,把 `pushCurrentBranch` 返回的
    ///    `LastSync` `writeProject` 回 manifest——只有真正确认送达的项目才
    ///    更新水位线,失败/跳过的项目不该被写进一个没发生过的"已同步"记录。
    ///    **这一步本身也可能失败**(比如 `projects/` 目录权限问题)——它必须
    ///    被单独的 `do/catch` 隔离,绝不能让一个回写失败带崩整批循环、甚至
    ///    跳过收尾的 workspace `syncUp`:push 已经真的把内容送到远端了(那条
    ///    `.uploaded` receipt 如实保留,不虚报成失败),回写失败是**另一件事**
    ///    ——本机 manifest 暂时没记下这次同步而已,追加一条独立的
    ///    `.failed(reason:)` receipt 让这件事同样可见(同一个 `itemId` 上
    ///    出现两条 receipt:一条 push 结果、一条回写结果,调用方按 `outcome`
    ///    分别判断,不是互斥的"一项一条")。
    /// 3. **收尾总是跑** `WorkspaceRepoService.syncUp`——不管 `items` 里有没有
    ///    显式列出 workspace 项、也不管前面项目的成败(包括回写失败):
    ///    manifest 刚被上一步改过(或者改的尝试失败了,但也可能有其它项目
    ///    成功改了),这次改动本身也需要被推上去,workspace 仓库是"永远最后
    ///    执行、总要跑"的收尾动作。同样立即 `progress` 回调。
    /// 4. 返回全部 receipts(每个成功执行的项目一条,回写失败再加一条,加上
    ///    workspace 收尾一条)——调用方自己数 ✓/✗,`SyncEngine` 不做"整体
    ///    成功/失败"这种二元判断,避免把「N 项成功 / M 项失败」的细节压扁成
    ///    一个布尔值。
    ///
    /// `now` 同 `ProjectSyncService`/`WorkspaceRepoService` 的注入时钟——默认
    /// `Date.init`,测试可以传固定值验证 receipt/`LastSync` 的时间戳,同时也
    /// 贯通到 `SyncEngine` 自己 catch 出来的 receipt 与收尾 `syncUp` 的
    /// commit message 里。
    public func executeUp(
        items: [SyncPlanItem], wipCommit: Bool, now: () -> Date = Date.init,
        progress: (SyncReceipt) -> Void
    ) throws -> [SyncReceipt] {
        var receipts: [SyncReceipt] = []
        let projects = try layout.loadProjects()
        let iso = ISO8601DateFormatter()

        for item in items where item.kind == .project && item.actionable {
            guard let project = projects[item.id] else {
                // plan 之后、执行之前,这个项目声明本身从 manifest 里消失了
                // (比如另一台机器并发删了它)——「绝不虚报」延伸到「绝不静默
                // 消失」:留一条 failed receipt 让这件事可见,而不是悄悄
                // continue,其余项目照常处理。
                let vanished = SyncReceipt(
                    itemId: item.id, kind: .project,
                    outcome: .failed(reason: "plan 后项目从 manifest 消失"),
                    at: iso.string(from: now())
                )
                receipts.append(vanished)
                progress(vanished)
                continue
            }

            let receipt: SyncReceipt
            var lastSync: LastSync?
            do {
                let localPath = try layout.effectiveLocalPath(
                    projectId: item.id, project: project, machineId: machineId)
                (receipt, lastSync) = try ProjectSyncService.pushCurrentBranch(
                    projectId: item.id, localPath: localPath, remoteURL: project.remote,
                    wipCommit: wipCommit, machineId: machineId, now: now
                )
            } catch {
                // 配置级错误(目录在 planUp 之后被删掉等)不上抛、不中断其余
                // 项目——如实包成这一项的 failed receipt。
                receipt = SyncReceipt(
                    itemId: item.id, kind: .project,
                    outcome: .failed(reason: "\(error)"), at: iso.string(from: now())
                )
            }

            receipts.append(receipt)
            progress(receipt)

            if let lastSync, case .uploaded = receipt.outcome {
                var updated = project
                updated.lastSync = lastSync
                do {
                    try layout.writeProject(id: item.id, updated)
                } catch {
                    // push 的 uploaded receipt 已经如实保留(远端确实收到了)
                    // ——回写 manifest 失败是独立的一件事,单独隔离出 do/catch,
                    // 追加一条 failed receipt,循环继续、收尾 workspace
                    // syncUp 照跑,不因为这一步失败带崩整批。
                    let writeBackFailure = SyncReceipt(
                        itemId: item.id, kind: .project,
                        outcome: .failed(reason: "push 成功但 manifest 回写失败: \(error)"),
                        at: iso.string(from: now())
                    )
                    receipts.append(writeBackFailure)
                    progress(writeBackFailure)
                }
            }
        }

        let message = "sync: \(machineId) \(iso.string(from: now()))"
        let workspaceReceipt = WorkspaceRepoService.syncUp(layout: layout, message: message, now: now)
        receipts.append(workspaceReceipt)
        progress(workspaceReceipt)

        return receipts
    }

    // MARK: - planDown

    /// 「本机有未同步变更,请先上行或手动处理」——`planDown`/`executeDown` 共用的
    /// 不可动用户仓库判词,brief 逐字规定。
    private static let downBlockedDetail = "本机有未同步变更,请先上行或手动处理"

    /// `planDown` 判定「clone」项时使用的 detail 文案——`executeDown` 用它(而不是
    /// 重新问一次 `FileManager` 现在这一刻目录还在不在)来判断这一项该走 clone
    /// 还是 pull。**这条纪律很关键**:`executeDown` 必须忠于 `planDown` 当时的
    /// 判定,不能在执行时重新探测"目录现在还在不在"——如果重新探测,"plan 时是
    /// pull 项(目录还在),执行前目录被意外删掉"这种环境被破坏的场景会被误当成
    /// "缺目录 → 直接 clone"悄悄"自愈"过去,而不是如实报告"这一项出问题了"
    /// (同 `executeUp` 对"plan 之后环境被破坏"的隔离纪律——那类问题该体现成
    /// 这一项的 `.failed`,不该被另一条本来是给"真的从没 checkout 过"用的逻辑
    /// 悄悄接管)。
    private static let cloneDownDetail = "本机缺目录,将 clone"

    /// 扫一遍「现在需要从远端收敛什么到本机」——先把 workspace 仓库自身拉新
    /// (manifest 是"目标状态"的唯一真相源,不刷新就是拿旧地图规划路线),再对每个
    /// 项目判断"本机离 manifest 记录的目标还有多远、能不能安全自动收敛"。
    ///
    /// **返回值比 brief 的单数组多一个位置**(controller 授权的调整):
    /// `(workspaceReceipt: SyncReceipt, items: [SyncPlanItem])`——第 0 步
    /// `WorkspaceRepoService.syncDown` 本身产出的回执不该被吞掉,调用方(UI)需要
    /// 知道"这次刷新 manifest 到底成没成"(比如离线时 `syncDown` 会 `.failed`,
    /// 但 plan 仍然照常用本机现有的 manifest 出清单——离线也该看得见"现在是什么
    /// 状态",只是这份清单可能不是最新的,`workspaceReceipt` 如实带着这个信息)。
    ///
    /// 每个项目的判定(brief + controller 裁决逐条对应):
    /// - `project.lastSync == nil`(这台/任何机器都还没为这个项目跑过一次成功的
    ///   上行同步)→ **不进 plan**——没有 manifest 记录的目标状态可收敛,"从远端
    ///   拉什么"这个问题在这里没有答案,列出来只会是无意义的噪音。
    /// - 本机缺目录(还没 checkout 过)→ `clone` 项,`actionable = true`。
    /// - 本机有目录,`last_sync.head == 本机 head` → 干净,**不进 plan**(和
    ///   `planUp` 的"干净不进 plan"是同一条纪律)。
    /// - head 不同,且本机不 dirty **且**本机没有独立进度(判定:本机 head 是
    ///   `last_sync.head` 的祖先——`WorkspaceGit.isAncestor`)→ `pull` 项,
    ///   `actionable = true`。
    /// - 本机 dirty,**或**本机有独立进度(不是祖先——分叉了,或者本机反而领先
    ///   manifest 记录)→ `actionable = false`,`detail` 如实说明"本机有未同步
    ///   变更,请先上行或手动处理"——**绝不自动动用户仓库**,这是 brief 反复强调
    ///   的红线:自动 pull/rebase/merge 一个有本机独立历史的仓库,轻则造出多余
    ///   的 merge commit,重则丢用户以为已经保存的工作。
    /// - `scan`(这里具体是 `WorkspaceGit.head`/`isDirty`/`isAncestor` 等)本身
    ///   抛错(目录存在但不是 git repo、git 不在 PATH 等**配置级**问题)→ 同
    ///   `planUp` 的纪律:进 plan 但 `actionable = false`,`detail` 带错误描述,
    ///   不让一个项目的扫描失败连累整个 `planDown` 抛出。
    public func planDown() throws -> (workspaceReceipt: SyncReceipt, items: [SyncPlanItem]) {
        // 第 0 步:刷新 manifest 本身。`.failed` 时(离线等)照常用本机现有的
        // manifest 出 plan——`workspaceReceipt` 如实带着这次刷新有没有成功。
        let workspaceReceipt = WorkspaceRepoService.syncDown(layout: layout, now: Date.init)

        var items: [SyncPlanItem] = []
        let projects = try layout.loadProjects()

        for (projectId, project) in projects.sorted(by: { $0.key < $1.key }) {
            guard let lastSync = project.lastSync else {
                continue // 从没同步过,没有 manifest 记录的目标状态可收敛。
            }

            do {
                let localPath = try layout.effectiveLocalPath(
                    projectId: projectId, project: project, machineId: machineId)

                var isDirectory: ObjCBool = false
                let exists = FileManager.default.fileExists(
                    atPath: localPath.path, isDirectory: &isDirectory)

                guard exists, isDirectory.boolValue else {
                    items.append(SyncPlanItem(
                        id: projectId, kind: .project, title: project.name,
                        detail: Self.cloneDownDetail, actionable: true
                    ))
                    continue
                }

                // **先纯本地读**,不碰网络——`head`/`isDirty` 都是读本地状态,
                // 大多数项目大多数时候都已经收敛(干净),这一步就该短路掉,不该
                // 为了"确认已经干净"这件事去逼一次 fetch(离线时会把满屏已收敛
                // 的干净项目都染成"扫描失败"的噪音,这正是 controller 裁决要
                // 堵上的反向虚报)。
                let localHead = try WorkspaceGit.head(at: localPath)
                let dirty = try WorkspaceGit.isDirty(at: localPath)

                // "干净"要求 head 对上 manifest **且** 工作区没有未提交变更——
                // 只比 head 会漏掉"没换 commit 但手上有没提交的编辑"这种情况
                // (同 `planUp` 干净判定 `status.dirty || status.ahead > 0` 的
                // 同一条纪律:dirty 本身就足以让这一项脱离"干净,不进 plan")。
                guard dirty || localHead != lastSync.head else {
                    continue // 干净,不进 plan——纯本地判定,不需要联网。
                }

                if dirty {
                    items.append(SyncPlanItem(
                        id: projectId, kind: .project, title: project.name,
                        detail: Self.downBlockedDetail, actionable: false
                    ))
                    continue
                }

                // 走到这里才是真正需要联网决策的分支:head 不同、且不 dirty,
                // 需要判断"本机是落后(能安全 pull)还是分叉/领先(不能动)"。
                // `last_sync.head` 可能是别的机器刚推上去、本机对象库里压根
                // 还没有的一个 commit——`fetch`(只更新 remote-tracking ref,
                // 不碰工作区/本地分支,不算"动用户仓库")先把它拉进本地对象库,
                // 后面的 `isAncestor` 才有得比。fetch 本身失败(离线/远端不可
                // 达)**不归到**外层"扫描失败"这种配置级错误的 catch 里,而是
                // 这一项单独的"无法联网核对"——如实呈现,不静默吞、也不误报
                // 成别的错误。
                do {
                    try WorkspaceGit.fetch(at: localPath)
                } catch {
                    items.append(SyncPlanItem(
                        id: projectId, kind: .project, title: project.name,
                        detail: "无法联网核对: \(error)", actionable: false
                    ))
                    continue
                }

                let localIsAncestor = try WorkspaceGit.isAncestor(
                    at: localPath, ancestor: localHead, descendant: lastSync.head)
                if localIsAncestor {
                    items.append(SyncPlanItem(
                        id: projectId, kind: .project, title: project.name,
                        detail: "落后 manifest 记录,将 pull", actionable: true
                    ))
                } else {
                    // 不是干净的"落后"关系——本机 dirty 已经在上面处理过,这里
                    // 走到的是"分叉了,或者本机反而领先 manifest 记录"——两种
                    // 情况都不该被自动 pull 覆盖,同样的 detail、同样不可动。
                    items.append(SyncPlanItem(
                        id: projectId, kind: .project, title: project.name,
                        detail: Self.downBlockedDetail, actionable: false
                    ))
                }
            } catch {
                items.append(SyncPlanItem(
                    id: projectId, kind: .project, title: project.name,
                    detail: "扫描失败: \(error)", actionable: false
                ))
            }
        }

        return (workspaceReceipt, items)
    }

    // MARK: - executeDown

    /// 执行一次下行:只处理 `items` 里 `actionable == true` 的项——`planDown`
    /// 已经把"不能自动动"的项过滤成 `actionable = false`,这里不重新判断,只管
    /// 执行。
    ///
    /// 两种 `.project` 项的执行方式(brief 逐字对应,单项失败不中断,同
    /// `executeUp` 的隔离纪律——包括 `SyncEngine` 自己的 `do/catch`)。**走哪条
    /// 路径由 `planDown` 当时的判定(`item.detail == Self.cloneDownDetail`)决定,
    /// 不在这里重新问一次 `FileManager` 现在这一刻目录还在不在**——`planDown`
    /// 判定"缺目录"是给"这个项目从没 checkout 过"这个场景用的;如果一个在 plan
    /// 时目录还在、判定为 pull 的项目,在 `executeDown` 真正跑之前目录被意外
    /// 删掉(环境被破坏),不该被这条"缺目录 → clone"逻辑悄悄接管、静默"自愈"
    /// 成一次 clone——那样会把"这里出问题了"这件事吞掉。忠于 plan 的判定,让
    /// `fetch`/`pullFastForward` 对着一个不存在的目录原样报错,落回下面的
    /// `.failed(reason:)`,如实呈现:
    /// - **clone 项**(`planDown` 判定本机缺目录):`WorkspaceGit.clone` 到
    ///   `effectiveLocalPath`,再 `git switch <last_sync.branch>`——clone 默认
    ///   签出的分支未必是 manifest 记录要收敛到的分支,显式切一次;已经在目标
    ///   分支上时 `switch` 是 no-op,幂等。
    /// - **pull 项**(本机有目录,落后且能安全收敛):`fetch` + `pullFastForward`
    ///   ——**绝不 rebase/merge 用户项目仓库**,ff 不了就原样失败,如实呈现给
    ///   调用方(`pullFastForward` 抛出的 `GitError` 本身已经带着 git 的原始
    ///   stderr,不需要这里再加工措辞)。
    ///
    /// **两条路径共用的收尾校验**(brief"校验语义注意"的裁决,统一适用于 clone
    /// 和 pull——两条路径都可能撞见"remote 比 manifest 新"这同一个现实:
    /// 另一台机器直接推了项目仓库,却没有(或还没来得及)把这次同步记进
    /// manifest):
    /// - 执行后本机 head **等于** `last_sync.head` → `.pulled(newHead:)`,最
    ///   常见的"干净收敛到 manifest 记录"的情况。
    /// - 本机 head **不等于** `last_sync.head`,但 `last_sync.head` 是本机新
    ///   head 的祖先(`WorkspaceGit.isAncestor`)→ 依然 `.pulled(newHead:)`——
    ///   这不是错误,是 remote 比 manifest 记录的更新(`git pull`/`clone` 天然
    ///   会拉到 origin 当下的实际 HEAD,不会自己停在 manifest 记的那个旧点上),
    ///   `newHead` 如实携带这个更靠前的真实值。
    /// - 两者都不是(既不相等、`last_sync.head` 也不是新 head 的祖先——分叉了,
    ///   或者本机诡异地"退步"了)→ `.failed(reason:)`,如实呈现,不能假装
    ///   成功。
    ///
    /// `now` 同 `executeUp` 的注入时钟。
    public func executeDown(
        items: [SyncPlanItem], now: () -> Date = Date.init,
        progress: (SyncReceipt) -> Void
    ) throws -> [SyncReceipt] {
        var receipts: [SyncReceipt] = []
        let projects = try layout.loadProjects()
        let iso = ISO8601DateFormatter()

        for item in items where item.kind == .project && item.actionable {
            guard let project = projects[item.id], let lastSync = project.lastSync else {
                // plan 之后、执行之前,这个项目声明(或它的 last_sync)从
                // manifest 里消失了——同 `executeUp` 的"绝不静默消失"纪律。
                let vanished = SyncReceipt(
                    itemId: item.id, kind: .project,
                    outcome: .failed(reason: "plan 后项目(或其 last_sync)从 manifest 消失"),
                    at: iso.string(from: now())
                )
                receipts.append(vanished)
                progress(vanished)
                continue
            }

            let receipt: SyncReceipt
            do {
                let localPath = try layout.effectiveLocalPath(
                    projectId: item.id, project: project, machineId: machineId)

                if item.detail == Self.cloneDownDetail {
                    // clone 项:clone 后切到 manifest 记录的目标分支。
                    try WorkspaceGit.clone(remoteURL: project.remote, to: localPath)
                    try WorkspaceGit.switchBranch(at: localPath, branch: lastSync.branch)
                } else {
                    // pull 项:fetch + pullFastForward,绝不 rebase/merge。忠于
                    // plan 的判定——即便这一刻目录已经不在了(plan 之后被删),
                    // 也不该悄悄改道去 clone,让 git 命令对着不存在的目录原样
                    // 报错,归入下面的 failed。
                    try WorkspaceGit.fetch(at: localPath)
                    try WorkspaceGit.pullFastForward(at: localPath)
                }

                let newHead = try WorkspaceGit.head(at: localPath)
                if newHead == lastSync.head {
                    receipt = SyncReceipt(
                        itemId: item.id, kind: .project,
                        outcome: .pulled(newHead: newHead), at: iso.string(from: now())
                    )
                } else if try WorkspaceGit.isAncestor(
                    at: localPath, ancestor: lastSync.head, descendant: newHead
                ) {
                    // remote 比 manifest 记录的更新——如实带上真实 newHead,
                    // 不是错误;这句"为什么超过"的解释放进 `detail`,不编造进
                    // `Outcome.pulled` 的关联值里(见 SyncReceipt.detail 文档)。
                    receipt = SyncReceipt(
                        itemId: item.id, kind: .project,
                        outcome: .pulled(newHead: newHead), at: iso.string(from: now()),
                        detail: "已超过 manifest 记录(remote 更新)"
                    )
                } else {
                    receipt = SyncReceipt(
                        itemId: item.id, kind: .project,
                        outcome: .failed(reason: "收敛后本机 head 与 manifest 记录既不相等也非其祖先关系"),
                        at: iso.string(from: now())
                    )
                }
            } catch {
                // 配置级/运行期错误(目录被删、ff 不了、clone 失败等)都不上抛、
                // 不中断其余项目——如实包成这一项的 failed receipt。
                receipt = SyncReceipt(
                    itemId: item.id, kind: .project,
                    outcome: .failed(reason: "\(error)"), at: iso.string(from: now())
                )
            }

            receipts.append(receipt)
            progress(receipt)
        }

        return receipts
    }
}
#endif
