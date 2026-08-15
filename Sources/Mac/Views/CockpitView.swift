#if os(macOS)
import SwiftUI

/// 驾驶舱(cockpit)—— [对账](docs/handbook/pendingcrew/concepts/cockpit.md) 的呈现面。
///
/// **临时窗口**(#542)：由 `MacThreePaneView` 以 overlay 叠在三栏之上,群聊那一栏**始终挂着**
/// ——关掉驾驶舱回到群聊,草稿和滚动位置原样还在(旧版整片换 NavigationSplitView,群聊被
/// 重建、草稿冲掉)。关闭是左上角那颗圆形叉(红绿灯红点的位置心智),Esc 同效。
/// 范围 = 侧栏选中的 crew(cockpit.md:per-crew 是沿 DAG 切一刀的过滤视图,存储仓库级一份)。
///
/// 两段(期望段已并进路线段 —— 人类原话「期望和 roadmap 我希望合二为一,以 roadmap 为主,
/// 在 roadmap 里像 map 一样操作」):
/// - **路线**:一张能缩放的地图,左阶段/分组/条目、右期望页正文(可就地编辑)。
/// - **任务**:活儿的三段式 glance,人类 Todo 与 task 账合流。
///
/// 全程 read-only(除期望页就地编辑):差靠人扫两栏读,**不判 drift、不重解析文档**
/// (cockpit.md 边界)。控制半边 = 看着差一步生 session。
struct CockpitView: View {
    @ObservedObject var runner: CrewSessionRunner
    @EnvironmentObject private var crewStore: CrewStore

    @State private var data: CockpitData?
    @State private var segment: Segment = .roadmap
    @State private var selectedNodeID: String?    // 期望:选中的 handbook 节点 id(页 relpath)

    enum Segment: String, CaseIterable, Identifiable {
        // 只剩两段：现状条以只读备注跟在期望页里（Todo #20）、总览段已删、Todo 并进任务段
        // （#542）、期望段并进路线段（人类：以 roadmap 为主，在 roadmap 里像 map 一样操作）。
        case roadmap = "路线", tasks = "任务"
        var id: String { rawValue }

        /// 旧入口写死过 rawValue（工具栏 Todo 深链、总览/期望深链），映射到并进来的新段，
        /// 免得老按钮点了没反应。
        static func resolve(_ raw: String) -> Segment? {
            if let seg = Segment(rawValue: raw) { return seg }
            switch raw {
            case "Todo": return .tasks
            case "总览", "期望", "现状": return .roadmap
            default: return nil
            }
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
        }
        .background(Theme.Palette.canvas)
        .task(id: crewStore.selectedDetail?.crew.workingDirectory ?? crewStore.selectedCrewId ?? "") {
            reload()
        }
        // 段深链（Todo #12）：工具栏 Todo 按钮等入口先写 runner.cockpitSegmentRequest
        // 再翻 showingCockpit —— 首开走 onAppear,已开着时走 onChange,两口都消费。
        .onAppear(perform: consumeSegmentRequest)
        .onChange(of: runner.cockpitSegmentRequest) { _, _ in consumeSegmentRequest() }
    }

    /// 消费一次性段深链请求：合法 rawValue → 切段;消费后清 nil。
    private func consumeSegmentRequest() {
        guard let raw = runner.cockpitSegmentRequest else { return }
        runner.cockpitSegmentRequest = nil
        if let seg = Segment.resolve(raw) { segment = seg }
    }

    // MARK: header(关闭 + crew + 分段)

    private var header: some View {
        HStack(spacing: 14) {
            // 左上角圆形叉 —— 临时窗口的关闭件,位置对齐 Mac 红绿灯红点的心智。
            // 样式走共用的玻璃白件(Todo #22),与各子窗口的关闭按钮同一颗。
            GlassCloseButton(action: { runner.showingCockpit = false },
                             help: "关闭驾驶舱（Esc）")
            VStack(alignment: .leading, spacing: 1) {
                Text("驾驶舱").font(Theme.Fonts.footnote.weight(.semibold))
                Text(crewStore.selectedDetail?.crew.title ?? crewStore.selectedCrew?.title ?? "—")
                    .font(Theme.Fonts.caption2).foregroundStyle(Theme.Palette.inkMuted)
            }
            Spacer(minLength: 12)
            Picker("", selection: $segment) {
                ForEach(Segment.allCases) { seg in Text(seg.rawValue).tag(seg) }
            }
            .pickerStyle(.segmented)
            .fixedSize()
            Spacer(minLength: 12)
            Button { reload() } label: { Image(systemName: "arrow.clockwise") }
                .buttonStyle(.borderless).help("重读账本")
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
    }

    // MARK: 分段路由

    @ViewBuilder private var content: some View {
        if segment == .tasks {
            // 任务段不 gate 在 data 上：人类 Todo 存在 app 数据目录，crew 没配账本时
            // 也该看得到（活跃 task 账同理，它在 ~/.claude 不在工作目录）。
            CockpitTasksGlance(
                data: data ?? CockpitLoader.empty,
                crewId: crewStore.selectedDetail?.crew.id ?? crewStore.selectedCrewId)
        } else if let data {
            switch segment {
            case .roadmap:
                CockpitRoadmapSegment(data: data, selection: $selectedNodeID, nav: handleNav)
            case .tasks:
                EmptyView()   // 上面已拦
            }
        } else {
            emptyState(crewStore.selectedDetail == nil
                ? "选一个 crew 看它的驾驶舱"
                : "此 crew 的工作目录里没有账本(docs/handbook · state · roadmap)。\n让某个 crew 的工作目录指向带这些账的仓库(比如大绿豆自己),驾驶舱就渲染真数据。")
        }
    }

    // MARK: 跨账跳转

    private func handleNav(_ nav: CockpitNav) {
        switch nav {
        case .page(let rel), .topic(let rel):
            // 期望段并进路线段后,跳一页期望 = 在路线地图上定位到那条并在右栏打开它
            //（地图会自动把它所在的阶段/分组展开）。现状备注跟在同一页里。
            selectedNodeID = rel; segment = .roadmap
        case .task:
            segment = .tasks
        }
    }

    // MARK: data

    private func reload() {
        guard let workdir = crewStore.selectedDetail?.crew.workingDirectory, !workdir.isEmpty else {
            data = nil; return
        }
        data = CockpitLoader.load(crewRoot: URL(fileURLWithPath: workdir))
    }

    // MARK: 控制半边 —— 已撤(人类 Todo #31)
    //
    // 原来这里有 `spawnBrief`:把某条现状的「期望 + 当前现状」当 brief 直接起一个 worker
    // session 去对齐(cockpit.md:terraform apply)。撤掉的原因是**数据源不可信**——现状账
    // `docs/state` 自 2026-07-26 起停更了两周,拿它派活等于照两周前的描述派真活。
    // 这不是砍能力,是等现状账重新跟得上代码再接回来;实现原样在 git 历史里。

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
