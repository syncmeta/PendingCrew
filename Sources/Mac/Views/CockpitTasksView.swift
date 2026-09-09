#if os(macOS)
import SwiftUI

/// 驾驶舱唯一正文：Agent 自己维护的计划与判断（Todo #46 / #81）。
///
/// 旧驾驶舱把仓库 roadmap、handbook/state、活跃 task 账和人类 Todo 混在一起；
/// 其中默认首页又依赖当前 crew 工作目录里的 `docs/roadmap.md`，文件不存在时整屏近乎
/// 空白。人类 2026-08-12 已拍板：驾驶舱不再做这些账的聚合器，只展示 Agent 自己的
/// 理解、计划和更新。仓库账与 Todo 仍各自在原来的入口存在，但不再冒充 Agent 的脑子。
///
/// 当前写入口是机长的 `plan_add` / `plan_update`。每条保留写入时间与最后更新时间；
/// 界面默认展示最新判断，点开才看这条计划的完整更新序列。
struct CockpitAgentMindView: View {
    let crewId: String
    /// 群聊里那颗「计划 #N」胶囊带过来的落点（人类 Todo #132/#133）。
    var planFocus: CockpitPresentation.PlanFocus? = nil

    /// 读回来的账 —— **按 crew 认领**（见 `CockpitPlanFeed`）。读改成异步之后，
    /// 迟到的结果不认、别的 crew 的行不显示。
    @State private var feed = CockpitPlanFeed()
    @State private var expanded: Set<Int> = []
    /// 只看这一条（人类 Todo #132/#133）。nil = 完整列表。
    @State private var focused: Int?
    /// 已经认领过的落点 token（见 `applyPlanFocusIfNeeded`）。
    @State private var appliedFocusToken: Int?

    private var plans: [CockpitPlanItem] { feed.plans(for: crewId) }

    /// 这次要显示的行。指到不存在的 #N 时回落成完整列表（见 `CockpitPlanFocusing`）。
    private var visibleItems: [CockpitTaskItem] {
        CockpitPlanFocusing.focused(planItems, number: focused, numberOf: Self.planNumber)
    }
    /// 真的落在单条上了吗 —— 「‹ 全部」只在这时出现。
    private var isFocused: Bool {
        CockpitPlanFocusing.isFocused(planItems, focused: visibleItems)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 26) {
                if plans.isEmpty {
                    emptyState
                } else {
                    if isFocused {
                        // 落点没砍掉列表能力 —— 这颗把它原样还回来（同 Todo 详细窗口）。
                        Button {
                            focused = nil
                        } label: {
                            Label("全部", systemImage: "chevron.left")
                                .font(Theme.Fonts.caption)
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(Theme.Palette.accent)
                        .help("回到完整的计划列表")
                    }
                    ForEach(CockpitTaskLedger.bands(visibleItems)) { group in
                        bandSection(group)
                    }
                }
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 22)
            .frame(maxWidth: 760, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Theme.Palette.canvas)
        // 群聊里点了「计划 #N」—— 落在那条上并把它摊开。
        //
        // **`onAppear` 与 `onChange` 都要挂**：`open(planNumber:)` 是先写落点再开
        // 驾驶舱，所以首次出现时值已经在了，`onChange` 一次都不会触发 —— 只挂
        // `onChange` 的话「关掉再点一颗胶囊」永远落不上，而这恰好是最常走的那条路。
        .onAppear { applyPlanFocusIfNeeded() }
        .onChange(of: planFocus) { _, _ in applyPlanFocusIfNeeded() }
        // 换 crew：落点作废。别的 crew 的 #N 是另一条计划，留着就是张冠李戴。
        .onChange(of: crewId) { _, _ in focused = nil }
        // `.task` 继承 MainActor —— 所以这里**一次磁盘 IO 都不能直接做**（人类 Todo #96）。
        // `CockpitPlanStore.list` 里是阻塞式 `flock(LOCK_EX)` + 整份 JSON 解码：
        // 只要有 helper 正在写这个 crew 的 .plan.lock，主线程就停在那儿等，
        // 而打开驾驶舱第一件事就是走这条路。改走 `listOffMain`，主线程只剩赋值。
        .task(id: crewId) {
            let requested = crewId
            await reload(requested)
            for await _ in CockpitPlanStore.shared.planChanges(crewId: requested) {
                if Task.isCancelled { return }
                await reload(requested)
            }
        }
    }

    /// 后台读一版，回主线程再决定认不认。
    ///
    /// 两道都要过：`Task.isCancelled`（切 crew 时 `.task(id:)` 会取消旧的那次）
    /// 和 `CockpitPlanFeed` 的 crew 认领（旧的那次**已经在路上**、取消标志还没来得及
    /// 生效时，结果照样会回来）。少一道，人就会在新 crew 的标题下看到上一个 crew 的作战板。
    private func reload(_ requested: String) async {
        let fresh = await CockpitPlanStore.shared.listOffMain(crewId: requested)
        guard !Task.isCancelled else { return }
        feed.apply(rows: fresh, requested: requested, current: crewId)
    }

    private var planItems: [CockpitTaskItem] {
        let now = Date()
        return CockpitPlan.newestFirst(plans).map { plan in
            let updated = Self.iso.date(from: plan.updatedAt)
            let statusLine = CockpitPlan.statusLine(
                statusRaw: plan.status, updated: updated, now: now)
            let blocker = plan.blockedBy.map { "卡在 \($0.label)" } ?? ""
            let latestThought = plan.updates.last.map { "最近：\($0.text)" } ?? ""
            let note = [blocker, latestThought, statusLine].first { !$0.isEmpty } ?? statusLine
            return CockpitTaskItem(
                id: Self.planPrefix + String(plan.number),
                title: plan.title,
                statusRaw: plan.status,
                origin: .captainPlan,
                updated: updated,
                badge: "#\(plan.number)",
                note: note)
        }
    }

    private static let planPrefix = "plan:"
    private static let iso = ISO8601DateFormatter()

    private func plan(for item: CockpitTaskItem) -> CockpitPlanItem? {
        guard let number = Self.planNumber(item) else { return nil }
        return plans.first { $0.number == number }
    }

    /// 条目 id（`plan:7`）→ 计划号。落点判定与行查找共用这一份，别再解析第二遍。
    private static func planNumber(_ item: CockpitTaskItem) -> Int? {
        Int(item.id.dropFirst(planPrefix.count))
    }

    /// 把外面送来的落点应用一次。**按 token 认领**（不是按 #N）：同一个 #N 连点
    /// 两次是两次请求，而人可能已经在驾驶舱里翻到别处去了；只比 #N 的话第二次
    /// 毫无反应。反过来，重建视图时重复应用同一个 token 也会把人自己按的
    /// 「‹ 全部」抹掉 —— 所以认领过就不再应用。
    private func applyPlanFocusIfNeeded() {
        guard let focus = planFocus, focus.token != appliedFocusToken else { return }
        appliedFocusToken = focus.token
        focused = focus.number
        expanded.insert(focus.number)
    }

    @ViewBuilder private func bandSection(_ group: CockpitBandGroup) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(group.band.rawValue)
                .font(Theme.Fonts.caption.weight(.semibold))
                .foregroundStyle(Theme.Palette.inkMuted)
                .padding(.bottom, 6)
            if group.items.isEmpty {
                Text("—")
                    .font(Theme.Fonts.footnote)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .padding(.vertical, 4)
            } else {
                ForEach(group.items) { row($0, band: group.band) }
            }
            if group.hiddenCount > 0 {
                Text("更早的 \(group.hiddenCount) 条不在这儿")
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .padding(.top, 6)
            }
        }
    }

    @ViewBuilder private func row(_ item: CockpitTaskItem, band: CockpitBand) -> some View {
        let stored = plan(for: item)
        let number = stored?.number ?? -1
        let isExpanded = expanded.contains(number)
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: 9) {
                Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(Theme.Palette.inkMuted)
                Circle()
                    .fill(dotColor(band))
                    .frame(width: 6, height: 6)
                    .offset(y: -1)
                Text(item.title)
                    .font(Theme.Fonts.footnote)
                    .foregroundStyle(band == .done ? Theme.Palette.inkMuted : Theme.Palette.ink)
                    .lineLimit(2)
                    .textSelection(.enabled)
                Spacer(minLength: 8)
                Text("Agent")
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.accent)
                    .padding(.horizontal, 5).padding(.vertical, 1)
                    .background(Theme.Palette.accent.opacity(0.12), in: Capsule())
                Text(item.badge)
                    .font(Theme.Fonts.caption2.monospaced())
                    .foregroundStyle(Theme.Palette.inkMuted)
            }
            if !item.note.isEmpty, !isExpanded {
                Text(item.note)
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .lineLimit(2)
                    .padding(.leading, 24)
            }
            if let stored, isExpanded {
                planDetail(stored)
            }
        }
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .onTapGesture {
            guard number >= 0 else { return }
            if isExpanded { expanded.remove(number) } else { expanded.insert(number) }
        }
    }

    @ViewBuilder private func planDetail(_ plan: CockpitPlanItem) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(CockpitPlan.statusLine(
                statusRaw: plan.status,
                updated: Self.iso.date(from: plan.updatedAt),
                now: Date()))
                .font(Theme.Fonts.caption2)
                .foregroundStyle(Theme.Palette.inkMuted)

            if let blocker = plan.blockedBy {
                Text("卡在 \(blocker.label)")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.amber)
            }

            if plan.updates.isEmpty {
                Text("Agent 还没有补充这条计划的判断或更新。")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.inkMuted)
            } else {
                ForEach(Array(plan.updates.reversed())) { update in
                    VStack(alignment: .leading, spacing: 3) {
                        Text(updateMeta(update))
                            .font(Theme.Fonts.caption2)
                            .foregroundStyle(Theme.Palette.inkMuted)
                        Text(update.text)
                            .font(Theme.Fonts.caption)
                            .foregroundStyle(Theme.Palette.ink)
                            .textSelection(.enabled)
                    }
                    .padding(.leading, 10)
                    .overlay(alignment: .leading) {
                        Rectangle()
                            .fill(Theme.Palette.accent.opacity(0.3))
                            .frame(width: 2)
                    }
                }
            }
        }
        .padding(.leading, 24)
        .padding(.top, 2)
    }

    private func updateMeta(_ update: CockpitPlanUpdate) -> String {
        var parts = [update.byName ?? "Agent"]
        if let date = Self.iso.date(from: update.createdAt) {
            parts.append(date.formatted(date: .abbreviated, time: .shortened))
        }
        if let raw = update.status, let status = CockpitPlan.status(raw) {
            parts.append(status.title)
        }
        return parts.joined(separator: " · ")
    }

    private func dotColor(_ band: CockpitBand) -> Color {
        switch band {
        case .doing: return Theme.Palette.accent
        case .next:  return Theme.Palette.inkMuted.opacity(0.5)
        case .done:  return Theme.Palette.success.opacity(0.6)
        }
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Agent 还没有记录计划和想法")
                .font(Theme.Fonts.headline)
            Text("这里不再读取仓库 roadmap、Todo 或 task 账。机长形成计划、判断或更新后，会通过作战板写进来。")
                .font(Theme.Fonts.caption)
                .foregroundStyle(Theme.Palette.inkMuted)
        }
        .frame(maxWidth: 520, alignment: .leading)
        .padding(.vertical, 40)
    }
}
#endif
