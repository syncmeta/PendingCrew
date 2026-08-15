#if os(macOS)
import SwiftUI

/// 路线段 —— 第四本账 `docs/roadmap.md` 渲成**一张能缩放的地图**。
///
/// 人类原话：「既然是 roadmap 就不要把所有东西都一个个列出来，map 还得有比例尺和聚合呢；
/// 现在就像中国地图把每个县都列出来。」所以左栏分四级放大，**一次只摊开需要的那一层**：
///
/// - **L1 阶段**：一张卡 = 阶段名 + 一句目标 + target + 聚合进度（n/m 就绪 + 进度条）。
///   不在做的阶段折成一行（点开才展开）。
/// - **L2 分组**：账里的 `### 组名`。在做的阶段默认展开**到这一层为止** —— 这就是缺的那把
///   比例尺：先看哪块在动，再决定放大看哪几条，**不默认摊条目**。
/// - **L3 条目**：点开某个组才列它的期望页条目（页名 + 现状色点）。
/// - **L4 正文**：点条目在右栏就地打开该期望页（可编辑），左侧地图保持可见、当前条目高亮 ——
///   「在 roadmap 里像 map 一样操作」。期望段因此并了进来，不再是独立一段。
///
/// 全程只读账本、**不判 drift**（cockpit.md 边界）：进度条只是把现状账里的 status 数了个数。
/// 控制半边（「生 session 补差」）已撤 —— 见 `CockpitPageView.stateNote` 的说明：现状账
/// 停更期间拿它派活等于拿旧描述派真活，等账跟上再接回来。
struct CockpitRoadmapSegment: View {
    let data: CockpitData
    /// 右栏正在看的期望页 relpath（也是地图上的高亮项）。由驾驶舱持有，跨段深链改它。
    @Binding var selection: String?
    let nav: (CockpitNav) -> Void

    /// 展开态。阶段 key = 阶段名；组 key = "阶段名\u{1}组名"。
    @State private var expandedPhases: Set<String> = []
    @State private var expandedGroups: Set<String> = []
    @State private var seededFor: String?      // 已按哪份账播过默认展开（换 crew 要重播）
    @State private var showsUnfiled = false

    var body: some View {
        if let roadmap = data.roadmap {
            CockpitSplit(leftWidth: 320) {
                mapColumn(roadmap)
            } right: {
                CockpitPageView(data: data, relpath: selection, nav: nav)
            }
            .onAppear { seed(roadmap) }
            .onChange(of: roadmap) { _, new in seed(new) }
            .onChange(of: selection) { _, new in reveal(new, in: roadmap) }
        } else {
            emptyGuide
        }
    }

    // MARK: - 左栏：地图

    private func mapColumn(_ roadmap: CockpitRoadmap) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 6) {
                if let mainline = mainline(roadmap.preamble) {
                    Text(mainline)
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.inkMuted)
                        .lineLimit(3)
                        .padding(.bottom, 4)
                }
                ForEach(roadmap.phases) { phase in
                    phaseCard(phase)
                }
                unfiledSection(roadmap)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 14)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Theme.Palette.canvas)
    }

    /// preamble 里给人看的那句「当前主线」—— 开头那行 `>` 是写给 captain 的格式约定，
    /// 不是给人读的，滤掉。剩下为空就整段不渲。
    private func mainline(_ preamble: String) -> String? {
        let body = preamble
            .components(separatedBy: "\n")
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix(">") }
            .joined(separator: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return body.isEmpty ? nil : body
    }

    // MARK: L1 阶段

    @ViewBuilder private func phaseCard(_ phase: CockpitPhase) -> some View {
        let open = expandedPhases.contains(phase.id)
        let progress = data.progress(phase.entries)
        VStack(alignment: .leading, spacing: 7) {
            Button { toggle(phase) } label: {
                VStack(alignment: .leading, spacing: 5) {
                    HStack(alignment: .firstTextBaseline, spacing: 7) {
                        Image(systemName: open ? "chevron.down" : "chevron.right")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(Theme.Palette.inkMuted)
                        Text(phase.name)
                            .font(Theme.Fonts.footnote.weight(.semibold))
                            .foregroundStyle(Theme.Palette.ink)
                            .lineLimit(1)
                        Spacer(minLength: 6)
                        if !phase.target.isEmpty {
                            Text(phase.target)
                                .font(Theme.Fonts.caption2.monospaced())
                                .foregroundStyle(Theme.Palette.inkMuted)
                        }
                    }
                    if !phase.goal.isEmpty {
                        Text(phase.goal)
                            .font(Theme.Fonts.caption)
                            .foregroundStyle(Theme.Palette.inkMuted)
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                    }
                    HStack(spacing: 8) {
                        CockpitProgressBar(fraction: progress.fraction,
                                           settledFraction: progress.settledFraction,
                                           color: phaseColor(phase))
                        Text(progress.isEmpty ? "—" : progress.label)
                            .font(Theme.Fonts.caption2.monospaced())
                            .foregroundStyle(Theme.Palette.inkMuted)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("\(bandName(phase)) · \(progress.longLabel)")

            if open {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(phase.groups) { group in
                        if group.isImplicit {
                            // 账里没写 `###` —— 没有比例尺可用，直接摊条目（老格式照旧）。
                            ForEach(Array(group.entries.enumerated()), id: \.offset) { _, entry in
                                entryRow(entry)
                            }
                        } else {
                            groupBlock(phase, group)
                        }
                    }
                    if phase.groups.isEmpty {
                        Text("（此阶段还没挂期望页）")
                            .font(Theme.Fonts.caption)
                            .foregroundStyle(Theme.Palette.inkMuted)
                            .padding(.leading, 16)
                    }
                }
                .padding(.top, 1)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 9)
        .background(Theme.Palette.surfaceMuted.opacity(open ? 1 : 0.55),
                    in: RoundedRectangle(cornerRadius: 10))
    }

    /// 阶段的 band（在做 / 接下来 / 做过）—— 走和任务段同一套状态词表，人拍板的 status 只读不判。
    private func bandName(_ phase: CockpitPhase) -> String {
        CockpitTaskLedger.band(phase.status)?.rawValue ?? "作废"
    }

    private func phaseColor(_ phase: CockpitPhase) -> Color {
        switch CockpitTaskLedger.band(phase.status) {
        case .doing: return Theme.Palette.accent
        case .done:  return Theme.Palette.success
        default:     return Theme.Palette.inkMuted
        }
    }

    // MARK: L2 分组（比例尺）

    @ViewBuilder private func groupBlock(_ phase: CockpitPhase, _ group: CockpitPhaseGroup) -> some View {
        let key = groupKey(phase, group)
        let open = expandedGroups.contains(key)
        let progress = data.progress(group.entries)
        VStack(alignment: .leading, spacing: 2) {
            Button { toggle(key) } label: {
                HStack(spacing: 7) {
                    Image(systemName: open ? "chevron.down" : "chevron.right")
                        .font(.system(size: 8, weight: .semibold))
                        .foregroundStyle(Theme.Palette.inkMuted)
                    Text(group.name)
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.ink)
                        .lineLimit(1)
                    Spacer(minLength: 6)
                    CockpitProgressBar(fraction: progress.fraction,
                                       settledFraction: progress.settledFraction,
                                       color: Theme.Palette.success)
                        .frame(width: 46)
                    Text(progress.label)
                        .font(Theme.Fonts.caption2.monospaced())
                        .foregroundStyle(Theme.Palette.inkMuted)
                }
                .padding(.vertical, 3)
                .padding(.leading, 14)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(progress.longLabel)
            if open {
                ForEach(Array(group.entries.enumerated()), id: \.offset) { _, entry in
                    entryRow(entry)
                }
                if group.entries.isEmpty {
                    Text("（这组还空着）")
                        .font(Theme.Fonts.caption2)
                        .foregroundStyle(Theme.Palette.inkMuted)
                        .padding(.leading, 30)
                        .padding(.vertical, 2)
                }
            }
        }
    }

    // MARK: L3 条目

    private func entryRow(_ entry: CockpitPhaseEntry) -> some View {
        // 存在性与现状都在加载期算好（原来这里每帧做一次同步 fileExists + O(n) 找 topic）。
        let exists = data.roadmapPagesPresent.contains(entry.relpath)
        let statusRaw = data.statusByRelpath[entry.relpath]
        let bucket = statusRaw.map(cockpitStatusBucket) ?? "unfiled"
        let selected = selection == entry.relpath
        return HStack(spacing: 6) {
            Button { selection = entry.relpath } label: {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Circle()
                        .fill(exists ? CockpitUI.color(bucket) : Theme.Palette.danger)
                        .frame(width: 6, height: 6)
                        .offset(y: -1)
                        .help(exists ? "现状：\(statusRaw ?? "未入账")" : "期望页不存在")
                    // 只显期望页末段名 —— 前缀是路径不是信息；点进去自然知道在哪。
                    Text(leafName(entry.relpath))
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(exists ? Theme.Palette.ink : Theme.Palette.danger)
                        .lineLimit(1)
                    if !entry.note.isEmpty {
                        Text(entry.note)
                            .font(Theme.Fonts.caption2)
                            .foregroundStyle(Theme.Palette.inkMuted)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 4)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
        .padding(.vertical, 3)
        .padding(.leading, 30)
        .padding(.trailing, 6)
        .background(selected ? Theme.Palette.accentBg : .clear,
                    in: RoundedRectangle(cornerRadius: 6))
    }

    private func leafName(_ relpath: String) -> String {
        relpath.split(separator: "/").last.map(String.init) ?? relpath
    }


    // MARK: 未挂路线（地图外的东西也得有地方去）

    /// 路线账没收编的两类：handbook 里有页但没进任何阶段、现状条压根没有期望页。
    /// 默认折成一行 —— 它们不是路线，但也不能因为并掉期望段就从界面上消失。
    @ViewBuilder private func unfiledSection(_ roadmap: CockpitRoadmap) -> some View {
        let filed = Set(roadmap.phases.flatMap(\.entries).map(\.relpath))
        let pages = allPages(data.handbookTree).filter { !filed.contains($0) }
        let pageSet = Set(pages)
        let orphans = data.topics.filter {
            !filed.contains($0.expectationRelpath) && !pageSet.contains($0.expectationRelpath)
        }
        if !pages.isEmpty || !orphans.isEmpty {
            VStack(alignment: .leading, spacing: 2) {
                Button { showsUnfiled.toggle() } label: {
                    HStack(spacing: 7) {
                        Image(systemName: showsUnfiled ? "chevron.down" : "chevron.right")
                            .font(.system(size: 8, weight: .semibold))
                        Text("未挂路线 \(pages.count + orphans.count)")
                            .font(Theme.Fonts.caption)
                        Spacer(minLength: 0)
                    }
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("有期望页 / 现状条，但没写进 docs/roadmap.md 的任何阶段")
                if showsUnfiled {
                    ForEach(pages, id: \.self) { page in
                        entryRow(CockpitPhaseEntry(relpath: page, note: ""))
                    }
                    ForEach(orphans) { topic in
                        orphanRow(topic)
                    }
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
        }
    }

    private func orphanRow(_ topic: CockpitTopic) -> some View {
        let id = CockpitPageView.orphanPrefix + topic.id
        return Button { selection = id } label: {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Circle().fill(CockpitUI.color(topic.status))
                    .frame(width: 6, height: 6).offset(y: -1)
                Text(topic.key)
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.ink)
                    .lineLimit(1)
                Text("缺期望页")
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.danger)
                Spacer(minLength: 4)
            }
            .padding(.vertical, 3)
            .padding(.leading, 30)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .background(selection == id ? Theme.Palette.accentBg : .clear,
                    in: RoundedRectangle(cornerRadius: 6))
    }

    private func allPages(_ nodes: [HandbookNode]) -> [String] {
        nodes.flatMap { n -> [String] in n.isPage ? [n.id] : allPages(n.children) }
    }

    // MARK: - 展开态

    private func groupKey(_ phase: CockpitPhase, _ group: CockpitPhaseGroup) -> String {
        "\(phase.name)\u{1}\(group.name)"
    }

    private func toggle(_ phase: CockpitPhase) {
        if expandedPhases.contains(phase.id) { expandedPhases.remove(phase.id) }
        else { expandedPhases.insert(phase.id) }
    }

    private func toggle(_ groupKey: String) {
        if expandedGroups.contains(groupKey) { expandedGroups.remove(groupKey) }
        else { expandedGroups.insert(groupKey) }
    }

    /// 默认缩放级别：在做的阶段展开**到分组为止**，其余折起来。组一律不预展开 ——
    /// 那正是「每个县都列出来」的老毛病。
    private func seed(_ roadmap: CockpitRoadmap) {
        let stamp = roadmap.phases.map(\.id).joined(separator: "\u{1}")
        guard seededFor != stamp else { return }
        seededFor = stamp
        expandedPhases = Set(roadmap.phases
            .filter { CockpitTaskLedger.band($0.status) == .doing }
            .map(\.id))
        expandedGroups = []
        reveal(selection, in: roadmap)
    }

    /// 跨账深链（`CockpitNav.page/.topic`）落到这里：把选中条目所在的阶段和组展开，
    /// 否则高亮藏在折叠层里，等于跳转没生效。
    private func reveal(_ relpath: String?, in roadmap: CockpitRoadmap) {
        guard let relpath, !relpath.hasPrefix(CockpitPageView.orphanPrefix) else { return }
        for phase in roadmap.phases {
            for group in phase.groups where group.entries.contains(where: { $0.relpath == relpath }) {
                expandedPhases.insert(phase.id)
                if !group.isImplicit { expandedGroups.insert(groupKey(phase, group)) }
            }
        }
    }

    // MARK: 空态引导

    private var emptyGuide: some View {
        VStack(spacing: 14) {
            Spacer()
            Text("这个仓库还没有路线账").font(Theme.Fonts.headline)
            Text("在仓库根建 docs/roadmap.md（人定骨架：主线 + 阶段 + 分组；captain 填血肉）。格式：")
                .font(Theme.Fonts.caption).foregroundStyle(Theme.Palette.inkMuted)
            Text("""
            > 路线账。阶段按文件顺序排先后。

            当前主线一段话。

            ## v1 收口
            status: doing
            target: 2026-08
            目标: 一句话阶段目标。
            ### 核心机制
            - pendingcrew/concepts/cockpit — 备注可选
            """)
            .font(Theme.Fonts.caption.monospaced())
            .padding(12)
            .background(Theme.Palette.surfaceMuted, in: RoundedRectangle(cornerRadius: 10))
            .frame(maxWidth: 460)
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.Palette.canvas)
    }
}

/// 地图上的刻度条 —— 细、无字，数字跟在旁边（密集统计不糊在脸上）。
/// 两档进度条（人类 Todo #31）：实心 = 已验证，同色浅影 = 做完待验（画在实心之下，
/// 露出来的那一截就是"待验"）。只传 `fraction` 时退化成单档，与改动前一致。
struct CockpitProgressBar: View {
    let fraction: Double
    /// 已验证 + 做完待验 的右缘；默认等于 `fraction`（无待验档）。
    var settledFraction: Double? = nil
    var color: Color = Theme.Palette.success
    var height: CGFloat = 4

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.Palette.hairline)
                Capsule().fill(color.opacity(0.3))
                    .frame(width: clamp(settledFraction ?? fraction) * geo.size.width)
                Capsule().fill(color)
                    .frame(width: clamp(fraction) * geo.size.width)
            }
        }
        .frame(height: height)
    }

    private func clamp(_ v: Double) -> Double { max(0, min(1, v)) }
}
#endif
