#if os(macOS)
import SwiftUI

/// 右栏 session 页内联的「待决策 / 待审批」可操作卡片
/// （spec 2026-06-08-pendingcrew-ask-approval-design §6 答复半边）。
///
/// 用户定调：**不另开面板** —— session 页本来就是「和这个 session 单聊」，待办就地答。
/// 这一层**事件驱动**订阅本地 `LocalApprovalStore`（去 1.5s 轮询）：app 侧 answer/decide
/// 即推、helper 子进程跨进程 raise pending 经目录监听补齐，过滤出**本 session**
/// （sessionId）的 pending：
/// - decision（`ask` 工具）→ 回复输入框 → `answer` → 解 helper 的 long-poll，agent 续跑。
/// - permission（权限 PreToolUse hook）→ `允许` / `拒绝` → `decide`。
///
/// 只在 BYOK 本地路径有内容（logged 模式 ask/审批走 edge interactions，那是另一条路）。
/// 多 session 单聊 + 未读角标 + 专门 reporter/captain session 是 chunk 2；这里是 v1 内核。
struct SessionApprovalCardsView: View {
    let crewId: String
    let sessionId: String

    private let store = LocalApprovalStore.shared
    @State private var pending: [ApprovalItem] = []
    @State private var drafts: [String: String] = [:]   // reqId → 回复草稿

    var body: some View {
        Group {
            if !pending.isEmpty {
                VStack(spacing: 8) {
                    ForEach(pending, id: \.id) { card($0) }
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.Palette.amberBg.opacity(0.12))
            }
        }
        // 事件驱动（去 1.5s 轮询）：先 refresh 一次兜住订阅前状态,再 for await store
        // 变更流 —— 本进程 answer/decide + helper 跨进程 raise(目录监听)都推一个 tick。
        // `.task(id:)` 切 session / 视图消失时取消,AsyncStream onTermination 退订上游。
        .task(id: sessionId) {
            refresh()
            for await _ in store.approvalChanges(crewId: crewId) {
                if Task.isCancelled { return }
                refresh()
            }
        }
    }

    @ViewBuilder
    private func card(_ item: ApprovalItem) -> some View {
        let isPermission = item.kind == "permission"
        VStack(alignment: .leading, spacing: 8) {
            Label(isPermission ? "待审批" : "待决策",
                  systemImage: isPermission ? "lock.shield" : "questionmark.bubble")
                .font(Theme.Fonts.caption.weight(.semibold))
                .foregroundStyle(Theme.Palette.amber)
            Text(item.summary)
                .font(Theme.Fonts.callout)
                .foregroundStyle(Theme.Palette.ink)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            if isPermission {
                HStack(spacing: 10) {
                    Button(role: .destructive) { decide(item, "deny") } label: {
                        Label("拒绝", systemImage: "xmark")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.bordered)
                    Button { decide(item, "allow") } label: {
                        Label("允许", systemImage: "checkmark")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                }
            } else {
                HStack(spacing: 6) {
                    TextField("答复…（agent 在等）", text: draftBinding(item.id), axis: .vertical)
                        .textFieldStyle(.roundedBorder)
                        .lineLimit(1...4)
                        .onSubmit { answer(item) }
                    Button("发送") { answer(item) }
                        .buttonStyle(.borderedProminent)
                        .disabled(draftEmpty(item.id))
                }
            }
        }
        .padding(14)
        .background(
            RoundedRectangle(cornerRadius: Theme.Metrics.cardRadius, style: .continuous)
                .fill(Theme.Palette.surfaceMuted)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Metrics.cardRadius, style: .continuous)
                .strokeBorder(Theme.Palette.amber.opacity(0.30), lineWidth: 0.6)
        )
        .padding(.horizontal, 12)
    }

    // MARK: - state

    private func draftBinding(_ id: String) -> Binding<String> {
        Binding(get: { drafts[id] ?? "" }, set: { drafts[id] = $0 })
    }
    private func draftEmpty(_ id: String) -> Bool {
        (drafts[id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
    private func refresh() {
        pending = store.pending(crewId: crewId).filter { $0.sessionId == sessionId }
    }
    private func answer(_ item: ApprovalItem) {
        let reply = (drafts[item.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !reply.isEmpty else { return }
        store.answer(crewId: crewId, id: item.id, reply: reply)
        drafts[item.id] = nil
        refresh()
    }
    private func decide(_ item: ApprovalItem, _ decision: String) {
        store.decide(crewId: crewId, id: item.id, decision: decision)
        refresh()
    }
}
#endif
