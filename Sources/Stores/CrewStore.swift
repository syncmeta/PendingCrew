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
/// **T5.2 重构**:Store 不再直接拼 HTTP 客户端,改成调
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
    /// captain MCP 交接命令待执行队列。helper 只入队；SessionHost 把它交给持有
    /// live runs 的 `CrewSessionRunner`，与 human UI 复用同一真实交接服务。
    @Published var captainHandoffRequests: [CaptainHandoffControlRequest] = []
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

    /// 机长 `change_workdir`（改工作目录 + 迁 agent 上下文）待执行队列。规划要看
    /// **在跑的 run**（谁还活着 / 谁在干活），那份状态只有 runner 有 —— 所以同
    /// `sessionOpsRequests`：这里只排队，执行 + 写应答归 `MacRootView` 接线。
    @Published var workdirChangeRequests: [WorkdirChangeRequest] = []

    /// 每个 crew 白板的**末条消息**（键缺失 = 该 crew 白板是空的）。侧栏两种视图
    /// 共用的单一数据源：时间流的排序键、行里的预览文案与行尾相对时间都读它。
    ///
    /// 以前这里是一个 `whiteboardRevision` 计数：目录一 tick 就 +1，侧栏行拿它当
    /// 依赖、在 **body 里同步读整份白板 JSON** 重新求值。28 个 crew × 4 次/秒 =
    /// 主线程每秒解析约 11 MB JSON（2026-08-17「开久了卡」的头号病根）。现在改成
    /// 「store 在后台按指纹门控算好、只在**真变了**时发布」：
    /// - SwiftUI body 里零磁盘 IO；
    /// - 值没变就不赋值 → 连 `objectWillChange` 都不发，无关文件的写不再让整个
    ///   侧栏（乃至所有观察 `CrewStore` 的视图）重渲染。
    @Published private(set) var lastWhiteboardMessages: [String: LocalWhiteboardMessage] = [:]

    /// 每个 crew 的**本 crew / 后代 crew**人类 Todo 未回应快照 —— Todo #71 起是
    /// 侧栏黄点的唯一数据源，Todo #73 再沿父边递归冒泡。与上面那份末条快照同样
    /// 是后台算好、只在真变了时发布，body 里零磁盘 IO。
    @Published private(set) var humanTodoAttention: [String: CrewHumanTodoAttention] = [:]

    /// 上面那份快照的算法（指纹门控，只有指纹变了的 crew 才重新解码）。
    private let lastMessageCache = CrewLastMessageCache(store: .shared)
    /// 人类 Todo 未回应数的同款指纹门控缓存（同一套 `FileFingerprintCache`）。
    private let humanTodoCache = CrewHumanTodoAttentionCache()
    /// 刷新在这条队列上做 —— stat + 解码都是磁盘 IO，不许上主线程；串行也顺带
    /// 保证目录 tick 密集时不会并发重入同一个 cache。
    private let lastMessageQueue = DispatchQueue(
        label: "com.pendingname.pendingcrew.sidebar-last-message", qos: .userInitiated)

    private unowned let appModel: AppModel

    /// crew-naming：机长 `rename_crew` 经 `LocalCrewControlStore` 写的待改名，
    /// 由 `LocalWhiteboardStore.directoryChanged`（跨进程目录监听）触发排空落地。
    /// 仅 macOS（本地 crew 的家）；只起一次。
    private var didStartRenameWatch = false
    private var renameWatch: AnyCancellable?
    /// 本进程 append（人类发送 / relay 搬入）的即时信号 —— 目录监听有 250ms 合流
    /// 窗口，自己发的消息不该等它。
    private var localWhiteboardWatch: AnyCancellable?

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
            // 列表变了（新 crew / 删掉的 crew）→ 末条消息快照跟着补齐一次，
            // 否则新 crew 的预览要等下一次目录 tick 才出现。
            refreshLastWhiteboardMessages()
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
        // Mac 本地态:LocalBackend 合成的「本机 (BYOK)」主体,供 CreateCrewSheet 拿
        // `subjects.first?.id` 当本地 crew 的 responsibleSubjectId。
        // iOS 无 backend（#63 后恒无）→ 留空。
        guard let backend = appModel.backend else { subjects = []; return }
        do {
            subjects = try await backend.listMySubjects()
        } catch {
            self.error = "加载 subject 列表失败：\(error.localizedDescription)"
        }
    }

    /// 机器清单 —— #63 第二期删掉跨端遥控整层之后**恒只有本机一台**。
    /// 侧栏按机器分组的数据源；`CreateCrewSheet` 据 `count > 1` 决定要不要显示
    /// machine 选择（本地态恒不显示）。这个属性留着不是为了「以后可能有别的机器」，
    /// 是因为侧栏分组和建 crew 页现在真读它。
    func refreshMachines() async {
        machines = [Machine(
            id: DeviceIdentity.current,
            kind: Machine.Kind.computer.rawValue,
            deviceId: DeviceIdentity.current,
            displayName: DeviceIdentity.displayName,
            flyMachineId: nil,
            status: "online",
            lastSeenAt: nil
        )]
    }

    // MARK: - Mutations

    /// 人类在本地 UI 手动改名。来源必须落成 human，避免机长把人类定好的名字
    /// 误当系统占位名反复提醒/覆盖。
    /// 侧栏「藏起来」被**拦住**时的那句说明（人手动藏一个还挂着活跃子 crew 的父）。
    /// 非 nil = 侧栏弹一句解释。纯 UI 瞬态，不落盘。
    @Published var hideBlockedNotice: String?

    /// 「藏它会连底下几个闲着的子 crew 一起藏」——落地前要人确认这一下。
    /// 非 nil = 侧栏弹确认。同样是 UI 瞬态。
    @Published var pendingSubtreeHide: PendingSubtreeHide?

    struct PendingSubtreeHide: Identifiable, Equatable {
        let crewId: String
        let crewTitle: String
        /// 会跟着一起从侧栏消失的子 crew 数（>0 才需要确认）。
        let alsoHiddenCount: Int

        var id: String { crewId }
    }

    /// 人手动把 crew 从侧栏藏起来。
    ///
    /// **只改人类界面的可见性** —— 不动父子边、不停 session、不碰白板；藏了的 crew
    /// 里的 session 照常干活、照常收发消息，`directory` / `contact` / 组织树一律
    /// 不受影响。
    ///
    /// 藏的正好是当前选中的那个（或它连带消失的子）时把选中清掉：人说的是「我不想
    /// 再看见它」，侧栏没了、中栏还开着它，那句话就只兑现了一半。
    func hideCrewFromUI(_ crewId: String) async {
        LocalCrewStore.shared.setManuallyHidden(crewId, hidden: true)
        await refreshList()
        if let selected = selectedCrewId,
           !CrewHiding.visible(crews).contains(where: { $0.id == selected }) {
            selectedCrewId = nil
        }
    }

    /// 从「已隐藏的群」列表把它取回侧栏。取回是**显式动作** —— 点进去看一眼不算
    /// （看和取回是两件事，见 `CrewHiding` / `CrewViewedStore`）。
    func unhideCrewFromUI(_ crewId: String) async {
        LocalCrewStore.shared.setManuallyHidden(crewId, hidden: false)
        await refreshList()
    }

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

    /// 清掉所有内存态（`LocalDataReset` 之类的整体重置路径用）。
    func reset() {
        crews = []
        lastWhiteboardMessages = [:]
        lastMessageCache.clear()
        humanTodoAttention = [:]
        humanTodoCache.clear()
        details = [:]
        subjects = []
        machines = []
        selectedCrewId = nil
        error = nil
    }

    // MARK: - Internals

    /// 拿当前生效的 backend;nil 时设 error 给 UI 显示。
    /// macOS 恒 `LocalBackend`;iOS 恒 nil（空壳）。
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
                    // 白板可能有变 → 后台按指纹门控重算侧栏末条消息快照（真变了才
                    // 发布）。以前这里是 `whiteboardRevision += 1`，等于让侧栏在
                    // 主线程 body 里把 28 份白板整份重解一遍。
                    self?.refreshLastWhiteboardMessages()
                    await self?.applyPendingRenames()
                    await self?.applyPendingAttentions()
                    await self?.drainPendingCommands()
                }
            }
        localWhiteboardWatch = LocalWhiteboardStore.shared.changes
            .sink { [weak self] _ in
                Task { @MainActor in self?.refreshLastWhiteboardMessages() }
            }
        #endif
    }

    /// 后台重算「每个 crew 的末条消息」，只在快照真变了时发布。
    ///
    /// 指纹门控在 `CrewLastMessageCache` 里：先 stat 一遍（28 次 stat），只有白板
    /// 文件的 mtime+size 真变了的那个 crew 才重新读+解码。主线程这边只剩一次
    /// 字典比较 + 可能的一次赋值。
    private func refreshLastWhiteboardMessages() {
        let crewIds = crews.map(\.id)
        let parentsByCrew = Dictionary(
            uniqueKeysWithValues: crews.map { ($0.id, $0.parentCrewIds) })
        let cache = lastMessageCache
        let todoCache = humanTodoCache
        lastMessageQueue.async { [weak self] in
            let snapshot = cache.refresh(crewIds: crewIds)
            // 人类 Todo 那本也搭同一趟车（Todo #62 ④）：两本账都落在同一个目录里，
            // 触发源是同一个 `directoryChanged` tick，各自指纹门控、各自只在真变了
            // 时发布。多的只是每个 crew 一次 stat。
            let todos = todoCache.refresh(
                crewIds: crewIds, parentsByCrew: parentsByCrew)
            Task { @MainActor in
                self?.publishLastWhiteboardMessages(snapshot)
                self?.publishHumanTodoAttention(todos)
            }
        }
    }

    /// 发布人类 Todo 未回应快照 —— 同样**相等就不赋值**（理由同上面那条：
    /// `CrewStore` 是整个侧栏的 `EnvironmentObject`）。
    private func publishHumanTodoAttention(
        _ snapshot: [String: CrewHumanTodoAttention]
    ) {
        guard humanTodoAttention != snapshot else { return }
        humanTodoAttention = snapshot
    }

    /// 发布快照 —— **相等就不赋值**。`@Published` 一赋值就发 `objectWillChange`，
    /// 而 `CrewStore` 是整个侧栏（乃至更多视图）的 `EnvironmentObject`：无关文件的
    /// 写若还照旧赋值，仍会 4 次/秒把它们全部重渲染一遍。
    private func publishLastWhiteboardMessages(_ snapshot: [String: LocalWhiteboardMessage]) {
        guard lastWhiteboardMessages != snapshot else { return }
        lastWhiteboardMessages = snapshot
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

    /// 排空旧 attention 文案变更，逐条落到 `LocalCrewStore.setAttention`（nil = 清除）；
    /// Todo #71 起它不再控制状态点，保留这条路只为旧会话和旧数据兼容。
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
            case "handoff_captain":
                let existing = cmd.sessionId?.isEmpty == false && cmd.runner == nil
                let fresh = cmd.sessionId == nil && (cmd.runner == "claude" || cmd.runner == "codex")
                guard existing || fresh else {
                    postSystemNotice(
                        crewId: cmd.crewId,
                        text: "机长交接被拒：控制命令模式含糊（必须二选一：现有 session 或显式 runner 新建）。旧机长保持不变。")
                    continue
                }
                captainHandoffRequests.append(CaptainHandoffControlRequest(
                    commandId: cmd.id, crewId: cmd.crewId,
                    requesterSessionId: cmd.requesterSessionId,
                    targetSessionId: cmd.sessionId, runner: cmd.runner,
                    model: cmd.model, effort: cmd.effort,
                    title: cmd.title, openingBrief: cmd.note))
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
            case "change_workdir":
                workdirChangeRequests.append(WorkdirChangeRequest(
                    commandId: cmd.id, crewId: cmd.crewId,
                    callerSessionId: cmd.sessionId, targetHint: cmd.title,
                    newPath: cmd.path ?? "",
                    includeChildren: cmd.includeChildren ?? true,
                    confirm: cmd.confirm ?? false))
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

/// `change_workdir` 命令排空后的一次待执行迁移。`confirm == false` = 只出预览。
/// `targetHint` 指本 crew 子树里的哪一个（nil = 本 crew）。
struct WorkdirChangeRequest: Equatable {
    let commandId: String
    /// 发起 crew —— 也是允许改动的**子树根**（不能拿它去动别的部门）。
    let crewId: String
    /// 发起的机长 session id：它自己不算「拦路的正在跑的 session」。
    let callerSessionId: String?
    let targetHint: String?
    let newPath: String
    let includeChildren: Bool
    let confirm: Bool
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

/// captain MCP 入队后交给 live runner 的明确二选一请求。
struct CaptainHandoffControlRequest: Equatable {
    let commandId: String
    let crewId: String
    let requesterSessionId: String?
    /// 非 nil = 现有成员模式；此时 runner 必须 nil，真实 kind 从会话账本读取。
    let targetSessionId: String?
    /// 非 nil = 新建模式；值只能为 claude/codex，targetSessionId 必须 nil。
    let runner: String?
    let model: String?
    let effort: String?
    let title: String?
    let openingBrief: String?
}
