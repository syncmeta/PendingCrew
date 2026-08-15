#if os(macOS)
import Foundation

/// 一个 crew 的事件驱动唤醒编排器（Phase 4b）。app 级**一个 crew 一个 waker**，
/// 该 crew 上的多个登录态 run 共用同一条 hub 连接。
///
/// 工序：
///   1. 订阅 `CrewRealtimeClient.events`（连 `/v1/realtime-hub/conv/:crewId`）。
///   2. 收到 `.changed`（「该 crew 有动静」信号，**不含** @列表）→ 对本 crew 上
///      每个登录态 run，拉一次 `GET /v1/sessions/:sessionId/inbox` 看自己的
///      mailbox（@我的权威源）。
///   3. 把 mailbox + 该 run 是否 `isBusy` 喂给纯函数 `CrewMailboxWakeLogic.decide`：
///      - 空闲 + 有@我的待处理项 → 经 `run.send()` 注入唤醒（claude=PTY 写文本；
///        codex=起 turn），注入后 `POST .../inbox/mark-delivered` 消费这些项。
///      - busy → noop，不打断、不消费；run 变 idle 后 runner 主动再 drain，
///        不依赖下一条 hub 事件。
///
/// 持有：由 `CrewSessionRunner` 在每个登录态 run 启动时 `ensure(...)` 懒建并按
/// crewId 缓存；最后一个该 crew 的 run 移除时由 runner `teardownIfIdle` 收掉。
/// 所有 IO 在 MainActor 上编排（runner / run 都是 MainActor），决策核心是纯函数。
///
/// best-effort：网络/拉取失败 console log 后忽略，下个 hub 事件自然重试 ——
/// 与 `CrewRelayAgent` 同策略。**活体 hub 帧 → 唤醒**这条路径留手动 E2E 验证。
@MainActor
final class CrewMailboxWaker {
    let crewId: String
    private let api: PendingCrewAPI
    private let client: CrewRealtimeClient
    private weak var runner: CrewSessionRunner?
    private var pump: Task<Void, Never>?
    /// tick 重入保护。重入不能直接丢：busy -> idle 回调可能正好撞在上一批 inbox
    /// 拉取期间，而现场未必还有下一条 hub 事件。记一位 `drainAgain`，当前批结束后
    /// 立刻再拉，保证 idle 这记不会漏。
    private var draining = false
    private var drainAgain = false

    init(crewId: String, api: PendingCrewAPI, client: CrewRealtimeClient, runner: CrewSessionRunner) {
        self.crewId = crewId
        self.api = api
        self.client = client
        self.runner = runner
    }

    /// 连上 hub + 起事件泵。幂等。
    func start() {
        guard pump == nil else { return }
        Task { await client.connect() }
        pump = Task { [weak self] in
            guard let self else { return }
            for await event in await self.client.events {
                if Task.isCancelled { return }
                if case .changed = event {
                    await self.drain()
                }
            }
        }
    }

    /// 关停：断 hub + 停事件泵。
    func stop() {
        pump?.cancel(); pump = nil
        Task { await client.close() }
    }

    /// 对本 crew 上每个登录态 run 拉一次 inbox，按决策注入 + mark-delivered。
    /// 收到 hub 事件即调；也可由外部「刚起 run，先补一次」主动调一次。
    func drain() async {
        guard !draining else {
            drainAgain = true
            return
        }
        draining = true
        repeat {
            drainAgain = false
            guard let runner else { break }
            // 本 crew、登录态（serverLink 存在 == 服务端 session，inbox 才有意义）、
            // 仍在跑的 run。BYOK/local-only run 没有服务端 mailbox，跳过。
            let targets = runner.runs.filter {
                $0.crewId == crewId && $0.status == .running && $0.hasServerSession
            }
            for run in targets { await wake(run) }
        } while drainAgain
        draining = false
    }

    /// 拉一个 run 的 inbox → 决策 → 注入 + mark-delivered。失败静默吞（best-effort）。
    private func wake(_ run: CrewSessionRun) async {
        let inbox: CrewSessionInbox
        do {
            inbox = try await api.getSessionInbox(sessionId: run.sessionId)
        } catch {
            NSLog("[CrewMailboxWaker] inbox \(run.sessionId) failed: \(error.localizedDescription)")
            return
        }
        // 项8:被 @ 唤醒的 claude 第一拍前置近期群聊上下文;codex 每轮 turn 自带
        // 未读白板 additionalContext,别重复塞 → 传空。
        // #490:近期上下文改从**共用游标**取未读(而非固定 suffix(15) 重发已注入过的
        // 历史),再 .suffix(15) 兜个上限;注入后推进同一游标,hook 路不再重复注入。
        let store = LocalWhiteboardStore.shared
        let cursor = WhiteboardCursor(
            directory: LocalWhiteboardStore.defaultDirectory,
            crewId: crewId, sessionId: run.sessionId)
        let unread: [LocalWhiteboardMessage] = run.kind == .claudeCode
            ? cursor.unread(in: store)
            : []
        // #543：上下文只放对该 session 可见的条目（@ 别人的定向不当「近期群聊」灌进来）。
        let recent = Array(CrewWhiteboardVisibility
            .visible(unread, to: run.sessionId, isCaptain: run.role == .captain)
            .suffix(15))
        switch CrewMailboxWakeLogic.decide(
            mailbox: inbox.mailbox, isBusy: run.backend.isBusy, recent: recent) {
        case .noop:
            return
        case let .inject(text, deliveredIds):
            run.send(text)
            // wake-resilience：注入不再立刻消费（修「假送达」——注入被模态菜单吞/
            // 进程假死也标成已送达,消息就此丢失）。改走回执：runner.confirmWake
            // 窗内采样目标工作态,确认到达才 mark-delivered + 推游标；失败则
            // 什么都不消费（mailbox 项留着,下个 hub 事件重投；游标不动,上下文
            // 下次照带）+ confirmWake 内部白板告警 @captain。
            runner?.confirmWake(run: run, crewId: crewId, onConfirmed: { [api] in
                if let last = unread.last {
                    cursor.advance(to: last, in: store)
                }
                Task {
                    do {
                        _ = try await api.markInboxDelivered(sessionId: run.sessionId, itemIds: deliveredIds)
                    } catch {
                        // mark 失败 → 这些项下个事件会被重新拉到 + 重复注入。可接受
                        // （定向消息重复一次好过漏掉）；console log 便于诊断。
                        NSLog("[CrewMailboxWaker] mark-delivered \(run.sessionId) failed: \(error.localizedDescription)")
                    }
                }
            })
        }
    }
}
#endif
