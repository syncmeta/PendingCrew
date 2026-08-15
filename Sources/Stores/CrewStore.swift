import Foundation
import SwiftUI
import Combine

/// Crew 列表 + detail 缓存。
///
/// 设计原则（AppModel 顶端注释 + spec v2 §11）：**不**回到 "10+ 散装
/// `@Published` 字典" 的老路。所有 detail 数据进 `details: [String: CrewDetail]`
/// —— 一个 crewId 对应一个结构化 `CrewDetail`。新加字段时改 `CrewDetail`
/// struct，不要在这里 / view 里到处摊。
///
/// **T5.2 重构**:Store 不再直接拼 `PendingCrewAPI`,改成调
/// `appModel.backend`(`PendingCrewBackend` protocol)。这样登录态 / BYOK
/// 两条路用同一份 store 逻辑;backend 实例换了,数据来源自动切。
/// `client()` 老函数没了 —— 用 `currentBackend()` 替代,失败时设
/// `error = "未配置 backend"` 让 UI 显空态。
@MainActor
final class CrewStore: ObservableObject {
    @Published private(set) var crews: [CrewSummary] = []
    @Published private(set) var details: [String: CrewDetail] = [:]
    @Published private(set) var subjects: [UserSubject] = []
    /// 本账号可用机器（CreateCrewSheet 据 `count > 1` 决定显不显示 machine 选择）。
    /// 本地态恒单元素 [本机]；登录态走 `GET /v1/machines`。
    @Published private(set) var machines: [Machine] = []
    @Published var selectedCrewId: String?
    @Published private(set) var loadingList: Bool = false
    @Published private(set) var loadingDetailIds: Set<String> = []
    @Published private(set) var loadingSubjects: Bool = false
    @Published var error: String?

    /// 建带 captain 的 crew 后，请求自动起机长（用户要的零摩擦：新建即启动 + 群里报到）。
    /// `MacThreePaneView` 观察这个队列（它持有 `CrewSessionRunner`），捕获后清空。
    /// 放 store 而非建 crew 的 sheet：sheet 建完即 dismiss，且拿不到 window 级的
    /// sessionRunner —— 信号经 store 冒泡给常驻的 three-pane 才能可靠落地。
    @Published var captainAutostartRequests: [CaptainAutostartRequest] = []

    /// `start_session` 命令排空后的待起会话队列 —— `MacRootView`（持
    /// `CrewSessionRunner`）观察它,逐条调 `runner.startForBrief`,起完清空。
    ///
    /// **数组而非单值**:单个 `@Published` 槽位在同一次目录监听 tick 里连续
    /// 两条 `start_session` 落地时会丢命令 —— SwiftUI 对同步的 `@Published`
    /// 赋值做合并,`.onChange` 只看得到最后一次赋值。数组 append 不丢、
    /// 消费方读完整体后清空；`captainAutostartRequests` 也使用同样的队列语义。
    @Published var sessionSpawnRequests: [SessionSpawnRequest] = []
    /// `set_profile` 命令排空后的待切换队列（同 sessionSpawnRequests 的数组语义,
    /// 防同 tick coalescing 丢命令）。`MacRootView` 观察执行。
    @Published var profileChangeRequests: [SessionProfileChangeRequest] = []
    /// `schedule_wakeup` 命令排空后的待登记队列。`MacRootView` 交给
    /// `CrewSessionRunner.scheduleWakeup`（持久化 + 定时器）。
    @Published var wakeupRequests: [SessionWakeupRequest] = []
    /// `crew_message` 投递后的待唤醒队列（目标 crew 机长）。`MacRootView` 找
    /// 目标机长 run 直投注入（idle 才注,busy 靠下轮白板注入）。
    @Published var crewMessageWakes: [CrewMessageWake] = []
    /// `listen` 命令排空后的待登记队列（群聊收听;#465）。`MacRootView` 交给
    /// `CrewSessionRunner.applyListen`（登记 + 白板观察 + 到期自动停）。
    @Published var listenRequests: [SessionListenRequest] = []

    /// 机长 session 操作（inspect_session / nudge_session / stop_session）待执行
    /// 队列。数组语义同 `sessionSpawnRequests`（防同 tick 丢命令）。执行 + 写应答
    /// 文件归 runner（`MacRootView` 观察接线）。
    @Published var sessionOpsRequests: [SessionOpsRequest] = []

    /// 白板目录变更计数（crew-sidebar-status spec §2 方案 A）。`directoryChanged`
    /// 每 tick +1；侧栏行（`CrewDAGNode`）读一下它，白板一变（本进程 append /
    /// helper 子进程跨进程写）就触发行重渲染，让同步读 `LocalWhiteboardStore` 的
    /// `lastMessagePreview` 重新求值 —— 否则最新消息预览只在点选等偶发重渲染时刷新。
    @Published private(set) var whiteboardRevision: Int = 0

    private unowned let appModel: AppModel

    /// crew-naming：机长 `rename_crew` 经 `LocalCrewControlStore` 写的待改名，
    /// 由 `LocalWhiteboardStore.directoryChanged`（跨进程目录监听）触发排空落地。
    /// 仅 macOS（本地 crew 的家）；只起一次。
    private var didStartRenameWatch = false
    private var renameWatch: AnyCancellable?

    init(appModel: AppModel) {
        self.appModel = appModel
    }

    // MARK: - Derived

    var selectedCrew: CrewSummary? {
        guard let id = selectedCrewId else { return nil }
        return crews.first(where: { $0.id == id })
    }

    var selectedDetail: CrewDetail? {
        guard let id = selectedCrewId else { return nil }
        return details[id]
    }

    // MARK: - Selection

    func selectCrew(_ id: String?) {
        selectedCrewId = id
        guard let id else { return }
        // detail 进 cache 之前，先发起 fetch（不 await）。view 会按
        // `details[id]` 的 nil / 非 nil 状态切空态 / 内容态。
        Task { await refreshDetail(id) }
    }

    // MARK: - Refresh

    func refreshList() async {
        startRenameWatchIfNeeded()   // crew-naming：首刷时挂上改名监听（仅 macOS，幂等）
        guard let backend = currentBackend() else { return }
        loadingList = true
        defer { loadingList = false }
        do {
            let result = try await backend.listCrews()
            crews = result
            // 选中项消失（比如刚被删）的话清掉。
            if let sel = selectedCrewId, !result.contains(where: { $0.id == sel }) {
                selectedCrewId = nil
            }
        } catch {
            self.error = "加载 crew 列表失败：\(error.localizedDescription)"
        }
    }

    func refreshDetail(_ crewId: String) async {
        guard let backend = currentBackend() else { return }
        loadingDetailIds.insert(crewId)
        defer { loadingDetailIds.remove(crewId) }
        do {
            let detail = try await backend.getCrew(crewId)
            details[crewId] = detail
        } catch {
            self.error = "加载 crew 详情失败：\(error.localizedDescription)"
        }
    }

    func refreshSubjects() async {
        loadingSubjects = true
        defer { loadingSubjects = false }
        // 身份是**账号级**概念 —— 登录态一律走 edge `/v1/me/subjects` 拿真实主体
        // (display_name / kind / userId)。**不能**经 `appModel.backend`:macOS 上
        // 它恒为 `LocalBackend`(本地为家),其 `listMySubjects` 返回写死的假主体
        // 「本机 (BYOK)」—— 于是登录后侧栏身份一直显示这个假名而非真实账号
        // (本 fix 的病根)。与 `refreshMachines()` 同构:authed → edge,否则本地兜底。
        if appModel.isAuthenticated {
            // 登录态只认 edge 真实主体,**绝不**回落本地假主体(否则又显示
            // 「本机 (BYOK)」)。拉取失败保留上次 subjects、只记 error,不挡 UI。
            guard let api = try? appModel.loggedAPIClient() else { return }
            do {
                subjects = try await api.listMySubjects()
                // 回填登录用户 id(群聊区分"自己的气泡")—— 真实主体带 userId。
                appModel.setCurrentUserIdIfResolved(subjects.first?.userId)
            } catch {
                self.error = "加载身份失败：\(error.localizedDescription)"
            }
            return
        }
        // 未登录(Mac 本地态):LocalBackend 合成的「本机 (BYOK)」假主体,供
        // CreateCrewSheet 拿 `subjects.first?.id` 当本地 crew 的 responsibleSubjectId。
        // iOS 未登录无 backend → 留空(WelcomeView 时态)。
        guard let backend = appModel.backend else { subjects = []; return }
        do {
            subjects = try await backend.listMySubjects()
        } catch {
            self.error = "加载 subject 列表失败：\(error.localizedDescription)"
        }
    }

    /// 拉机器清单：本机始终在；登录态额外并入账号其它机器（edge `GET /v1/machines`）。
    /// 机器是账号概念 → 走 edge，**不**经 crew backend（crew 仍本地优先）。
    /// 失败静默回落「仅本机」—— 机器清单只是侧栏分组数据源，拉不到不挡用。
    func refreshMachines() async {
        let localMachine = Machine(
            id: DeviceIdentity.current,
            kind: Machine.Kind.computer.rawValue,
            deviceId: DeviceIdentity.current,
            displayName: DeviceIdentity.displayName,
            flyMachineId: nil,
            status: "online",
            lastSeenAt: nil
        )
        guard appModel.isAuthenticated, let api = try? appModel.loggedAPIClient() else {
            machines = [localMachine]
            return
        }
        do {
            let remote = try await api.listMachines()
            // edge 已含本机行（deviceId == 本机）则用 edge 行，否则把合成本机置顶补上。
            machines = remote.contains { $0.deviceId == DeviceIdentity.current }
                ? remote
                : [localMachine] + remote
        } catch {
            machines = [localMachine]
        }
    }

    /// 把本机真注册进账号 machine 表（edge，幂等 upsert by device_id）。登录后调。
    /// 失败静默 —— best-effort，不挡用户用 app。未登录直接跳过（返回 nil）。
    @discardableResult
    func registerSelfMachine() async -> String? {
        guard appModel.isAuthenticated, let api = try? appModel.loggedAPIClient() else {
            return nil
        }
        return try? await api.registerSelfMachine()
    }

    // MARK: - Mutations

    /// 人类在本地 UI 手动改名。来源必须落成 human，避免机长把人类定好的名字
    /// 误当系统占位名反复提醒/覆盖。
    func renameCrewFromUI(_ crewId: String, title: String) async {
        LocalCrewStore.shared.setTitle(crewId, title, source: .human)
        await refreshList()
        await refreshDetail(crewId)
    }

    /// 创建 crew。新 crew 进 `crews` 头部，**不抢当前视图**（人类 Todo #40）。
    /// 返回值是新 crew 的 summary —— caller 决定要不要做后续操作
    /// （比如关 sheet）。
    @discardableResult
    func createCrew(_ request: CreateCrewRequest,
                    autostartCaptain: Bool = true) async throws -> CreateCrewResponse {
        guard let backend = currentBackend() else {
            throw PendingCrewBackendError.notAuthenticated
        }
        let response = try await backend.createCrew(request)
        // 刷新列表 + detail —— 避免靠 client 自己拼 CrewSummary 出 bug
        // （edge 端将来加字段时会自动跟上）。
        await refreshList()
        await refreshDetail(response.crewId)
        // 建完**不**自动选中：机长后台 `create_child_crew` 和人手动新建都会走这里，
        // 抢走用户正在看的群聊很打断（Todo #40）。新 crew 靠侧栏列表刷新出现，
        // 用户自己点。唯一例外：当前一个 crew 都没选中（首次建第一个 crew）时选中它，
        // 否则界面会停在空白页。
        if selectedCrewId == nil {
            selectedCrewId = response.crewId
        }
        // 建完请求自动起机长（detail 已 refresh，three-pane 观察到就拉起 captain）。
        // 当前建 crew 必带 systemGenerated captain；用 detail.captain 兜底判断，
        // 防将来出现无 captain 的 crew 也误触发。
        if autostartCaptain, details[response.crewId]?.captain != nil {
            captainAutostartRequests.append(CaptainAutostartRequest(
                crewId: response.crewId,
                childTitle: details[response.crewId]?.crew.title ?? response.crewId))
        }
        return response
    }

    /// 把 `crewId` 挂到父 crew `parentCrewId` 之下(本地 DAG 父边)。
    /// 成功后刷新列表(侧栏树重算)+ 两端 detail。禁环错误向上抛给 UI 提示。
    func attachParent(crewId: String, parentCrewId: String) async throws {
        guard let backend = currentBackend() else {
            throw PendingCrewBackendError.notAuthenticated
        }
        try await backend.attachParent(
            crewId: crewId, parentCrewId: parentCrewId, childKeepsBps: 10000)
        await refreshList()
        await refreshDetail(crewId)
        await refreshDetail(parentCrewId)
    }

    /// 解绑 `crewId` 的父边 `parentCrewId`。成功后刷新列表 + 两端 detail。
    func detachParent(crewId: String, parentCrewId: String) async throws {
        guard let backend = currentBackend() else {
            throw PendingCrewBackendError.notAuthenticated
        }
        try await backend.detachParent(crewId: crewId, parentCrewId: parentCrewId)
        await refreshList()
        await refreshDetail(crewId)
        await refreshDetail(parentCrewId)
    }

    // MARK: - Auth lifecycle

    /// 用户退出登录时调，清掉所有内存态。`AppModel.clearAuth` 时一并调。
    func reset() {
        crews = []
        details = [:]
        subjects = []
        machines = []
        selectedCrewId = nil
        error = nil
    }

    // MARK: - Internals

    /// 拿当前生效的 backend;nil 时设 error 给 UI 显示。
    /// 唯一的"两态共用"入口 —— EdgeBackend / LocalBackend 切换在这里自动发生。
    private func currentBackend() -> PendingCrewBackend? {
        guard let backend = appModel.backend else {
            self.error = "未配置 backend(凭据缺失)"
            return nil
        }
        return backend
    }

    // MARK: - crew-naming：机长改名落地（仅 macOS）

    /// 挂上改名监听（幂等，仅 macOS）。机长 `rename_crew` 经离线 helper 把待改名
    /// 写进共享 `--dir`（= 白板目录）→ app 的 `directoryChanged` 跨进程目录监听触发
    /// → 排空落地到 `LocalCrewStore` + 刷新侧栏。启动先排空一次，兜住 app 没开时写入的改名。
    /// `setTitle` 写的是 `local-crews.json`（另一目录），不触发白板目录监听 → 不成环。
    private func startRenameWatchIfNeeded() {
        #if os(macOS)
        guard !didStartRenameWatch else { return }
        didStartRenameWatch = true
        LocalWhiteboardStore.shared.startWatching()
        Task {
            await applyPendingRenames()
            await applyPendingAttentions()
            await drainPendingCommands()
        }
        renameWatch = LocalWhiteboardStore.shared.directoryChanged
            .sink { [weak self] in
                Task { @MainActor in
                    // 白板有变 → 计数 +1，驱动侧栏最新消息预览重渲染（spec §2）。
                    self?.whiteboardRevision += 1
                    await self?.applyPendingRenames()
                    await self?.applyPendingAttentions()
                    await self?.drainPendingCommands()
                }
            }
        #endif
    }

    /// 排空 `LocalCrewControlStore` 的待改名，逐条落到 `LocalCrewStore`；有变更才刷新列表。
    private func applyPendingRenames() async {
        let renames = LocalCrewControlStore.shared.drainRenames()
        guard !renames.isEmpty else { return }
        for r in renames {
            LocalCrewStore.shared.setTitle(r.crewId, r.title, source: .captain)
        }
        await refreshList()
    }

    /// 排空 attention 变更（机长 `raise_attention` / `clear_attention`，crew-sidebar-status
    /// spec §3），逐条落到 `LocalCrewStore.setAttention`（nil = 熄灭）；有变更才刷新列表
    /// （`crews` 重发布 → 侧栏头像黄点亮/灭）。
    private func applyPendingAttentions() async {
        let changes = LocalCrewControlStore.shared.drainAttentions()
        guard !changes.isEmpty else { return }
        for ch in changes { LocalCrewStore.shared.setAttention(ch.crewId, reason: ch.reason) }
        await refreshList()
    }

    // MARK: - crew-comms：机长命令排空（start_session / create_child_crew）

    /// 排空机长命令通道，逐条执行。`start_session` → 追加到
    /// `sessionSpawnRequests`（`MacRootView` 观察数组、逐条起、清空）；
    /// `create_child_crew` → 通过既有 `createCrew` +
    /// `attachParent` 建子 crew（自动带出 `captainAutostartRequests`）+
    /// 父白板回执一行。与 `applyPendingRenames` 同一 tick 触发
    /// （`directoryChanged` 跨进程目录监听）。
    func drainPendingCommands() async {
        let cmds = LocalCrewControlStore.shared.drainCommands()
        guard !cmds.isEmpty else { return }
        for cmd in cmds {
            switch cmd.kind {
            case "start_session":
                guard let isolation = cmd.isolation else {
                    postSystemNotice(
                        crewId: cmd.crewId,
                        text: "起 session 失败：缺少工作区选择。请让机长重新调用 start_session，并明确 isolation=true/false。")
                    continue
                }
                sessionSpawnRequests.append(SessionSpawnRequest(
                    crewId: cmd.crewId, brief: cmd.brief,
                    runner: cmd.runner, isolation: isolation,
                    model: cmd.model, effort: cmd.effort, title: cmd.title))
            case "set_profile":
                guard let sid = cmd.sessionId else { break }
                profileChangeRequests.append(SessionProfileChangeRequest(
                    crewId: cmd.crewId, sessionId: sid, model: cmd.model, effort: cmd.effort))
            case "schedule_wakeup":
                guard let sid = cmd.sessionId, let fireAt = cmd.fireAt else { break }
                wakeupRequests.append(SessionWakeupRequest(
                    id: cmd.id, crewId: cmd.crewId, sessionId: sid,
                    fireAt: fireAt, note: cmd.note ?? ""))
            case "listen":
                guard let sid = cmd.sessionId else { break }
                listenRequests.append(SessionListenRequest(
                    crewId: cmd.crewId, sessionId: sid,
                    until: cmd.fireAt, senders: cmd.senders, off: cmd.off ?? false))
            case "inspect_session":
                guard let sid = cmd.sessionId else { break }
                sessionOpsRequests.append(SessionOpsRequest(
                    commandId: cmd.id, crewId: cmd.crewId, requesterSessionId: nil,
                    targetSessionId: sid, input: nil, stopReason: nil))
            case "nudge_session":
                guard let sid = cmd.sessionId else { break }
                sessionOpsRequests.append(SessionOpsRequest(
                    commandId: cmd.id, crewId: cmd.crewId, requesterSessionId: nil,
                    targetSessionId: sid, input: cmd.note ?? "", stopReason: nil))
            case "stop_session":
                guard let sid = cmd.sessionId,
                      let requester = cmd.requesterSessionId,
                      let reason = cmd.note else { break }
                sessionOpsRequests.append(SessionOpsRequest(
                    commandId: cmd.id, crewId: cmd.crewId, requesterSessionId: requester,
                    targetSessionId: sid, input: nil, stopReason: reason))
            case "crew_message":
                executeCrewMessage(cmd)
            case "create_child_crew":
                await executeCreateChildCrew(cmd)
            case "adopt_crew":
                await executeAdoptCrew(cmd)
            case "release_crew":
                await executeReleaseCrew(cmd)
            case "create_parent_crew":
                await executeCreateParentCrew(cmd)
            case "adopt_parent":
                await executeAdoptParent(cmd)
            default:
                break
            }
        }
    }

    /// 执行一条 `create_child_crew` 命令：继承父
    /// `workingDirectory`/`captainAgentKind`/`machineId`/`responsibleSubjectId`
    /// 建子 crew → 挂父边 → 父白板回执一行。全部失败路径都落一行回执，
    /// 不静默吞（机长看不到 app 里的错误提示，只能靠白板知道命令没成）。
    private func executeCreateChildCrew(_ cmd: CrewCommand) async {
        let parentId = cmd.crewId
        // 父 crew 的 workingDirectory 只在 detail（CrewBody）里,summary 没有这个字段 ——
        // 需要时才 fetch,不常驻占内存。取不到 detail 就现拉一次再读。
        if details[parentId] == nil {
            await refreshDetail(parentId)
        }
        guard let parentSummary = crews.first(where: { $0.id == parentId }),
              let parentDetail = details[parentId] else {
            postSystemNotice(crewId: parentId, text: "建子 crew 被拒：找不到父 crew 信息。")
            return
        }
        guard let wd = parentDetail.crew.workingDirectory, !wd.isEmpty else {
            postSystemNotice(crewId: parentId, text: "建子 crew 被拒：父 crew 无工作目录。")
            return
        }
        do {
            let request = CreateCrewRequest.make(
                responsibleSubjectId: parentSummary.responsibleSubjectId,
                title: cmd.title,
                machineId: parentSummary.machineId,
                workingDirectory: wd,
                captainAgentKind: parentSummary.captainAgentKind,
                initialTitleSource: cmd.title == nil ? .placeholder : .captain,
                captain: .systemGenerated(templateName: nil))
            // 子 crew 要在挂父边 + brief 白板留痕之后再起机长，确保它首轮拿到
            // 完整组织关系和开场任务；因此这里抑制 createCrew 的普通自动报到。
            let resp = try await createCrew(request, autostartCaptain: false)
            let childTitle = cmd.title ?? resp.crewId
            // attachParent 单独 catch：createCrew 已成功，子 crew 真实存在（子机长
            // 仍会带 brief 自动启动），只是没挂上父 DAG 边——回执必须如实说。
            var attachFailure: Error?
            do {
                try await attachParent(crewId: resp.crewId, parentCrewId: parentId)
            } catch {
                attachFailure = error
            }

            let autostart = CaptainAutostartRequest(
                crewId: resp.crewId, brief: cmd.brief,
                sourceCrewId: parentId, childTitle: childTitle)
            // 人类可追溯的任务原文先落盘，并 @ 子机长；随后才发布启动请求。
            do {
                try LocalWhiteboardStore.shared.appendSessionMessageReportingFailure(
                    crewId: resp.crewId,
                    sessionId: cmd.sessionId ?? "captain-\(parentId)",
                    text: cmd.brief,
                    category: "progress",
                    senderName: orgActorLabel(parentId),
                    mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)],
                    senderKind: "captain")
            } catch {
                if let receipt = autostart.deliveryFailureReceipt(
                    reason: "写入子 crew 白板失败：\(error.localizedDescription)") {
                    postSystemNotice(crewId: receipt.crewId, text: receipt.text)
                }
            }

            guard details[resp.crewId]?.captain != nil else {
                if let receipt = autostart.deliveryFailureReceipt(reason: "没有可启动的子机长") {
                    postSystemNotice(crewId: receipt.crewId, text: receipt.text)
                }
                return
            }
            captainAutostartRequests.append(autostart)

            if let attachFailure {
                postSystemNotice(
                    crewId: parentId,
                    text: "子 crew「\(childTitle)」已建出，但挂接父级失败：\(attachFailure.localizedDescription)。需在侧栏手动挂；开场任务仍会交给子机长。")
            } else {
                postSystemNotice(
                    crewId: parentId,
                    text: "已建子 crew「\(childTitle)」，开场任务已写入子群，子机长将自动接手。")
            }
        } catch {
            postSystemNotice(crewId: parentId, text: "建子 crew 失败：\(error.localizedDescription)")
        }
    }

    // MARK: - 组织架构调整命令（#22/#25：收编/摘出/建父/认父）
    //
    // 权限模型（人类 Todo #25 定调）：**上级控制下级,平级互不控制**。
    // - 向下：adopt_crew 收编（把顶层/无关 crew 挂到自己名下,是建立上下级的唯一
    //   「抓取」动作）;release_crew 只能动**自己的直系子**（摘顶层/转挂另一直系子）。
    // - 向上：create_parent_crew 建父、adopt_parent 认父（自愿挂靠）。
    // 环检测在 `LocalCrewStore.adopt/release`;每次结构变更**两边 crew
    // 的群聊都落回执**——谁动了架构要可见,失败也回执不静默吞。

    /// 结构变更回执的操作者署名：「<crew名>·机长」。
    private func orgActorLabel(_ crewId: String) -> String {
        "\(LocalCrewStore.shared.title(of: crewId) ?? crewId)·机长"
    }

    /// 架构调整在另一块白板留下的通知由真实操作者署名。`sessionId` 是各
    /// `enqueue*` 已携带的来源字段，不能只落盘后丢掉；缺失仅兼容旧命令文件。
    private func postOrgActorNotice(crewId: String, actorCrewId: String,
                                    sessionId: String?, text: String) {
        LocalWhiteboardStore.shared.appendSessionMessage(
            crewId: crewId,
            sessionId: sessionId ?? "captain-\(actorCrewId)",
            text: text,
            senderName: orgActorLabel(actorCrewId),
            senderKind: "captain")
    }

    /// 解析失败时的候选清单（全机 crew 标签）。
    private func orgCandidatesListing() -> String {
        let all = LocalCrewStore.shared.allCrewTitles()
        guard !all.isEmpty else { return "本机还没有其它 crew。" }
        return "本机 crew：" + all.map { "「\($0.title)」" }.joined(separator: "、") + "。"
    }

    /// 收编：把顶层/无关 crew 挂到发起 crew 名下当直系子（父边 additive,若对方
    /// 另有上级则保留 —— 收编不抢人,只加一条汇报线）。
    private func executeAdoptCrew(_ cmd: CrewCommand) async {
        let store = LocalCrewStore.shared
        let hint = cmd.title ?? ""
        guard let target = store.resolveAnyCrew(hint: hint) else {
            postSystemNotice(crewId: cmd.crewId,
                             text: "收编未执行：找不到 crew「\(hint)」或有歧义。\(orgCandidatesListing())")
            return
        }
        guard target != cmd.crewId else {
            postSystemNotice(crewId: cmd.crewId, text: "收编未执行：不能收编自己。")
            return
        }
        let targetTitle = store.title(of: target) ?? target
        guard !store.parentIds(of: target).contains(cmd.crewId) else {
            postSystemNotice(crewId: cmd.crewId, text: "「\(targetTitle)」已经是直系子，无需收编。")
            return
        }
        do {
            let hadOtherParents = !store.parentIds(of: target).isEmpty
            try store.adopt(crewId: target, underParent: cmd.crewId)
            let keep = hadOtherParents ? "（它原有的上级保留,现在多一条汇报线）" : ""
            postSystemNotice(crewId: cmd.crewId,
                             text: "已收编「\(targetTitle)」为子部门\(keep)。")
            postOrgActorNotice(
                crewId: target, actorCrewId: cmd.crewId, sessionId: cmd.sessionId,
                text: "本 crew 已被「\(store.title(of: cmd.crewId) ?? cmd.crewId)」收编为子部门。")
            await refreshAfterOrgChange([cmd.crewId, target])
        } catch {
            postSystemNotice(crewId: cmd.crewId, text: "收编「\(targetTitle)」失败：\(error.localizedDescription)")
        }
    }

    /// 摘出/转挂直系子。目的地也必须是自己的直系子（上级只在自己名下调整）。
    private func executeReleaseCrew(_ cmd: CrewCommand) async {
        let store = LocalCrewStore.shared
        let hint = cmd.title ?? ""
        guard let child = store.resolveChild(of: cmd.crewId, hint: hint) else {
            let kids = store.children(of: cmd.crewId)
            let listing = kids.isEmpty ? "本 crew 目前没有子 crew。"
                : "现有直系子：" + kids.map { "「\($0.title)」" }.joined(separator: "、") + "。"
            postSystemNotice(crewId: cmd.crewId,
                             text: "调整未执行：找不到直系子「\(hint)」或有歧义。\(listing)")
            return
        }
        let childTitle = store.title(of: child) ?? child
        var dest: String?
        if let destHint = cmd.note {
            guard let resolved = store.resolveChild(of: cmd.crewId, hint: destHint) else {
                postSystemNotice(crewId: cmd.crewId,
                                 text: "转挂未执行：目的地「\(destHint)」不是本 crew 的直系子。")
                return
            }
            dest = resolved
        }
        do {
            try store.release(crewId: child, from: cmd.crewId, to: dest)
            if let dest {
                let destTitle = store.title(of: dest) ?? dest
                postSystemNotice(crewId: cmd.crewId,
                                 text: "已把「\(childTitle)」转挂到「\(destTitle)」名下。")
                postOrgActorNotice(
                    crewId: child, actorCrewId: cmd.crewId, sessionId: cmd.sessionId,
                    text: "本 crew 已由「\(destTitle)」接管，原上级「\(store.title(of: cmd.crewId) ?? cmd.crewId)」。")
                postOrgActorNotice(
                    crewId: dest, actorCrewId: cmd.crewId, sessionId: cmd.sessionId,
                    text: "「\(childTitle)」已转挂到本 crew 名下。")
                await refreshAfterOrgChange([cmd.crewId, child, dest])
            } else {
                postSystemNotice(crewId: cmd.crewId,
                                 text: "已把「\(childTitle)」摘出到顶层。")
                postOrgActorNotice(
                    crewId: child, actorCrewId: cmd.crewId, sessionId: cmd.sessionId,
                    text: "本 crew 已脱离「\(store.title(of: cmd.crewId) ?? cmd.crewId)」回到顶层。")
                await refreshAfterOrgChange([cmd.crewId, child])
            }
        } catch {
            postSystemNotice(crewId: cmd.crewId, text: "调整「\(childTitle)」失败：\(error.localizedDescription)")
        }
    }

    /// 建父：新建一个顶层 crew（继承发起 crew 的目录/机长类型,自动起父机长）,
    /// 再把发起 crew 挂进去。
    private func executeCreateParentCrew(_ cmd: CrewCommand) async {
        let store = LocalCrewStore.shared
        let selfId = cmd.crewId
        if details[selfId] == nil { await refreshDetail(selfId) }
        guard let selfSummary = crews.first(where: { $0.id == selfId }),
              let selfDetail = details[selfId] else {
            postSystemNotice(crewId: selfId, text: "建父 crew 被拒：找不到本 crew 信息。")
            return
        }
        guard let wd = selfDetail.crew.workingDirectory, !wd.isEmpty else {
            postSystemNotice(crewId: selfId, text: "建父 crew 被拒：本 crew 无工作目录。")
            return
        }
        do {
            let request = CreateCrewRequest.make(
                responsibleSubjectId: selfSummary.responsibleSubjectId,
                title: cmd.title,
                machineId: selfSummary.machineId,
                workingDirectory: wd,
                captainAgentKind: selfSummary.captainAgentKind,
                initialTitleSource: cmd.title == nil ? .placeholder : .captain,
                captain: .systemGenerated(templateName: nil))
            let resp = try await createCrew(request) // 既有：内部会追加 captainAutostartRequests
            try store.adopt(crewId: selfId, underParent: resp.crewId)
            let parentTitle = store.title(of: resp.crewId) ?? cmd.title ?? resp.crewId
            postSystemNotice(crewId: selfId,
                             text: "已新建父 crew「\(parentTitle)」，本 crew 已挂为其子部门，父机长将自动报到。")
            postOrgActorNotice(
                crewId: resp.crewId, actorCrewId: selfId, sessionId: cmd.sessionId,
                text: "本 crew 由「\(store.title(of: selfId) ?? selfId)」创建并认作上级，你是它的父部门。")
            await refreshAfterOrgChange([selfId, resp.crewId])
        } catch {
            postSystemNotice(crewId: selfId, text: "建父 crew 失败：\(error.localizedDescription)")
        }
    }

    /// 认父：把现有 crew 认作发起 crew 的父（自愿挂靠,父边 additive）。
    private func executeAdoptParent(_ cmd: CrewCommand) async {
        let store = LocalCrewStore.shared
        let hint = cmd.title ?? ""
        guard let parent = store.resolveAnyCrew(hint: hint) else {
            postSystemNotice(crewId: cmd.crewId,
                             text: "认父未执行：找不到 crew「\(hint)」或有歧义。\(orgCandidatesListing())")
            return
        }
        guard parent != cmd.crewId else {
            postSystemNotice(crewId: cmd.crewId, text: "认父未执行：不能认自己当父。")
            return
        }
        let parentTitle = store.title(of: parent) ?? parent
        guard !store.parentIds(of: cmd.crewId).contains(parent) else {
            postSystemNotice(crewId: cmd.crewId, text: "「\(parentTitle)」已经是本 crew 的父，无需再认。")
            return
        }
        do {
            try store.adopt(crewId: cmd.crewId, underParent: parent)
            postSystemNotice(crewId: cmd.crewId,
                             text: "已认「\(parentTitle)」为父部门。")
            postOrgActorNotice(
                crewId: parent, actorCrewId: cmd.crewId, sessionId: cmd.sessionId,
                text: "「\(store.title(of: cmd.crewId) ?? cmd.crewId)」已认本 crew 为上级，成为你的子部门。")
            await refreshAfterOrgChange([cmd.crewId, parent])
        } catch {
            postSystemNotice(crewId: cmd.crewId, text: "认父「\(parentTitle)」失败：\(error.localizedDescription)")
        }
    }

    /// 结构变更后的统一刷新：列表（侧栏树重算）+ 涉事 crew 的 detail。
    private func refreshAfterOrgChange(_ crewIds: [String]) async {
        await refreshList()
        for id in crewIds { await refreshDetail(id) }
    }

    /// 执行一条机长跨 crew 消息（汇报线;#463 组织能力）。DAG 解析在这（helper
    /// 读不到 crew store）：to_parent → 所有直系父;to_child → resolveChild
    /// 按 标签/id/唯一前缀 解析,歧义或无匹配回执现有子 crew 清单（别投错部门）。
    /// 投递 = 写目标 crew 白板（署名「<源crew名>·机长」,@captain）+ 唤醒请求
    /// 交 `crewMessageWakes`（MacRootView 找目标机长 run 直投注入）。
    /// 全部失败路径都回执源 crew 白板,不静默吞。
    private func executeCrewMessage(_ cmd: CrewCommand) {
        let store = LocalCrewStore.shared
        let sourceTitle = store.title(of: cmd.crewId) ?? cmd.crewId
        var targets: [String] = []
        switch cmd.direction {
        case "to_parent":
            targets = store.parentIds(of: cmd.crewId)
            if targets.isEmpty {
                postSystemNotice(crewId: cmd.crewId, text: "向上汇报未送出：本 crew 已是根，没有父 crew。")
                return
            }
        case "to_child":
            guard let hint = cmd.title, let child = store.resolveChild(of: cmd.crewId, hint: hint) else {
                let kids = store.children(of: cmd.crewId)
                let listing = kids.isEmpty ? "本 crew 目前没有子 crew。"
                    : "现有子 crew：" + kids.map { "「\($0.title)」" }.joined(separator: "、") + "。"
                postSystemNotice(crewId: cmd.crewId,
                                 text: "消息未送出：找不到子 crew「\(cmd.title ?? "?")」或有歧义。\(listing)")
                return
            }
            targets = [child]
        default:
            return
        }
        let label = "\(sourceTitle)·机长"
        for target in targets {
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: target, sessionId: cmd.sessionId ?? "captain-\(cmd.crewId)",
                text: cmd.brief, category: "report", senderName: label,
                mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)])
            crewMessageWakes.append(CrewMessageWake(
                targetCrewId: target, text: cmd.brief, senderLabel: label))
            postSystemNotice(
                crewId: cmd.crewId,
                text: "已送达「\(store.title(of: target) ?? target)」群聊。")
        }
    }

    /// 往 crew 白板发一行系统回执。`LocalWhiteboardStore` 没有单独的
    /// "系统消息" API —— 复用 `appendSessionMessage`（senderKind "session"），
    /// `senderName` 标成「系统」区分于真实 session/机长发言，`sessionId` 用固定
    /// 哨兵值（不对应任何真实 session，纯展示用）。
    func postSystemNotice(crewId: String, text: String) {
        LocalWhiteboardStore.shared.appendSessionMessage(
            crewId: crewId, sessionId: "system", text: text, senderName: "系统")
    }
}

/// `start_session` 命令排空后的一次待起会话请求。`MacRootView` 观察
/// `CrewStore.sessionSpawnRequests` 数组，逐条调 `runner.startForBrief`。
/// `crew_message` 投递后的一次目标机长唤醒（汇报线;#463）。
struct CrewMessageWake: Equatable {
    let targetCrewId: String
    let text: String
    let senderLabel: String
}

/// `listen` 命令排空后的一次收听登记请求（群聊收听;#465）。`off == true` 时
/// until/senders 无意义（撤销该 session 的收听）。
struct SessionListenRequest: Equatable {
    let crewId: String
    let sessionId: String
    let until: String?
    let senders: [String]?
    let off: Bool
}

/// `inspect_session` / `nudge_session` / `stop_session` 命令排空后的一次机长操作。
/// `stopReason != nil` = stop；否则 `input == nil` = inspect，非 nil = nudge。
/// `commandId` 用于写应答文件（helper 侧 long-poll `takeCommandResponse`）。
struct SessionOpsRequest: Equatable {
    let commandId: String
    let crewId: String
    let requesterSessionId: String?
    let targetSessionId: String
    let input: String?
    let stopReason: String?
}

/// `set_profile` 命令排空后的一次待切换请求（session 自切模型/effort）。
struct SessionProfileChangeRequest: Equatable {
    let crewId: String
    let sessionId: String
    let model: String?
    let effort: String?
}

/// `schedule_wakeup` 命令排空后的一次待登记唤醒。
struct SessionWakeupRequest: Equatable {
    let id: String
    let crewId: String
    let sessionId: String
    let fireAt: String   // ISO8601
    let note: String
}

struct SessionSpawnRequest: Equatable {
    let crewId: String
    let brief: String
    /// nil = 随 crew `captainAgentKind`；"claude"/"codex" 覆盖。
    let runner: String?
    /// true = 独立 worktree；false/nil = 共享 crew 目录。
    let isolation: Bool
    /// 模型别名/slug；nil = 对应 runner 默认。
    var model: String? = nil
    /// thinking effort；nil = runner 默认。
    var effort: String? = nil
    /// 机长传的精简 title（≤18 字概括，作 session 显示名）；nil = 从 brief 兜底。
    var title: String? = nil
}
