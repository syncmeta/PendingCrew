#if os(macOS)
import SwiftUI

/// 驾驶舱(cockpit)—— Agent 当前计划与判断的呈现面（Todo #46 / #81）。
///
/// **临时窗口**(#542)：由 `MacThreePaneView` 以 overlay 叠在三栏之上,群聊那一栏**始终挂着**
/// ——关掉驾驶舱回到群聊,草稿和滚动位置原样还在(旧版整片换 NavigationSplitView,群聊被
/// 重建、草稿冲掉)。关闭是左上角那颗圆形叉(红绿灯红点的位置心智),Esc 同效。
/// 范围 = 侧栏选中的 crew。仓库 roadmap/handbook/state、Todo 与 task 账仍各自存在，
/// 但不再占据驾驶舱；这里仅呈现 `CockpitPlanStore` 里 Agent 自己写下的计划与更新。
struct CockpitView: View {
    /// 关掉自己。**刻意只收一个闭包，不收 `CrewSessionRunner`**（人类 Todo #96）：
    /// 驾驶舱除了「关掉我」以外不需要 runner 的任何东西，而 `@ObservedObject` 一挂上，
    /// runner 每一次 `objectWillChange`（session 状态、输出、心跳，一秒好几次）
    /// 都会让整个驾驶舱重算一遍。
    let onClose: () -> Void
    @EnvironmentObject private var crewStore: CrewStore

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
        }
        .background(Theme.Palette.canvas)
    }

    // MARK: header

    private var header: some View {
        HStack(spacing: 14) {
            // 左上角圆形叉 —— 临时窗口的关闭件,位置对齐 Mac 红绿灯红点的心智。
            // 样式走共用的玻璃白件(Todo #22),与各子窗口的关闭按钮同一颗。
            GlassCloseButton(action: onClose, help: "关闭驾驶舱（Esc）")
            VStack(alignment: .leading, spacing: 1) {
                Text("驾驶舱").font(Theme.Fonts.footnote.weight(.semibold))
                Text("\(crewStore.selectedDetail?.crew.title ?? crewStore.selectedCrew?.title ?? "—") · Agent 的计划与想法")
                    .font(Theme.Fonts.caption2).foregroundStyle(Theme.Palette.inkMuted)
            }
            Spacer(minLength: 12)
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
    }

    // MARK: 正文

    @ViewBuilder private var content: some View {
        if let crewId = crewStore.selectedDetail?.crew.id ?? crewStore.selectedCrewId {
            CockpitAgentMindView(crewId: crewId)
        } else {
            emptyState("选一个 crew 看 Agent 的计划与想法")
        }
    }

    private func emptyState(_ text: String) -> some View {
        VStack {
            Spacer()
            Text(text)
                .foregroundStyle(Theme.Palette.inkMuted)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

// MARK: - 跨账跳转意图

enum CockpitNav {
    case page(String)    // 期望 handbook 页 relpath
    case topic(String)   // 现状 topic(用 expectationRelpath 寻)
    case task(String)    // task id
}

// MARK: - 共享小件

/// 驾驶舱内的左右分栏 —— 用 GeometryReader 按**可用宽度**切。内容无 min 宽需求,所以永远
/// 等于可用宽、绝不向 NavigationSplitView detail 列要更多宽把 sidebar 顶出去(HSplitView 两侧
/// minWidth 相加会顶 sidebar/toolbar 出屏,见 tech-debt「inspector 被切」、memory「detail ideal 太贪」)。
struct CockpitSplit<L: View, R: View>: View {
    var leftWidth: CGFloat? = nil    // 固定左宽(右弹性);期望/现状用
    var rightWidth: CGFloat? = nil   // 固定右宽(左弹性);任务看板用
    @ViewBuilder var left: () -> L
    @ViewBuilder var right: () -> R

    var body: some View {
        GeometryReader { geo in
            let total = geo.size.width
            let (lw, rw) = split(total)
            HStack(spacing: 0) {
                left().frame(width: lw)
                Divider()
                right().frame(width: rw)
            }
        }
    }

    /// 左 + 右 + divider 恒等于 total —— 永不外溢。
    private func split(_ total: CGFloat) -> (CGFloat, CGFloat) {
        let d: CGFloat = 1
        if let l = leftWidth {
            let lw = max(160, min(l, total - 220))   // 左固定,但至少给右 220
            return (lw, max(0, total - lw - d))
        }
        if let r = rightWidth {
            let rw = max(160, min(r, total - 260))   // 右固定,但至少给左 260
            return (max(0, total - rw - d), rw)
        }
        return ((total - d) / 2, (total - d) / 2)
    }
}

/// status 色块徽章。
struct StatusBadge: View {
    let raw: String
    var body: some View {
        Text(raw.isEmpty ? "?" : raw)
            .font(.caption2.weight(.medium))
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(CockpitUI.color(raw).opacity(0.18), in: Capsule())
            .foregroundStyle(CockpitUI.color(raw))
    }
}

struct CockpitTaskMiniCard: View {
    let task: CockpitTask
    var body: some View {
        HStack(spacing: 8) {
            StatusBadge(raw: task.status)
            VStack(alignment: .leading, spacing: 1) {
                Text(task.title.isEmpty ? task.id : task.title).font(.callout).lineLimit(1)
                    .foregroundStyle(Theme.Palette.ink)
                Text(task.id).font(.caption2.monospaced()).foregroundStyle(Theme.Palette.inkMuted)
            }
            Spacer(minLength: 0)
        }
        .padding(10)
        .background(Theme.Palette.surfaceMuted, in: RoundedRectangle(cornerRadius: 8))
        .contentShape(Rectangle())
    }
}

/// status → 颜色(纯展示,无判 drift)。桶值幂等,传 raw 或 bucket 都行。
enum CockpitUI {
    static func color(_ raw: String) -> Color {
        switch cockpitStatusBucket(raw) {
        case "done": return Theme.Palette.success
        case "pending-qa": return Theme.Palette.gold
        case "partial": return Theme.Palette.amber
        case "planned": return Theme.Palette.inkMuted
        case "dropped": return Theme.Palette.danger
        default: return Theme.Palette.inkMuted
        }
    }
}
#endif
