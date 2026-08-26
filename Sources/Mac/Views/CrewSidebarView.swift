#if os(macOS)
import SwiftUI

/// 左栏：crew 列表 + toolbar（新建 / 刷新）。
///
/// MVP 阶段是平铺 list —— DAG 树形 sidebar 留下次（spec v2 §12.1 说明）。
/// `runtimeLocation` 用一个 systemImage + 缩写标签提示用户这是 local /
/// peer / fly。
struct CrewSidebarView: View {
    @EnvironmentObject private var crewStore: CrewStore
    @State private var showingCreateSheet = false
    /// 行右键「在这下面建子 crew」选中的父 crew（非 nil = 开子 crew 表单）。
    /// sheet 挂在侧栏顶层而不是行上：List 行会被回收，挂行上的表单可能被顶掉。
    @State private var childCrewTarget: CrewChildCreationTarget?
    @State private var showingWorkspaceSync = false
    /// 折叠起来的机器分组 id 集合（默认全展开）。
    @State private var collapsedMachineIds: Set<String> = []
    /// 长期职责的唯一所有者（spec §6）—— 用量监视归它持有，这里只观察。
    @EnvironmentObject private var sessionHost: SessionHost
    @ObservedObject private var quota = QuotaCenter.shared
    /// 层级 / 时间流（Todo #50）。写 UserDefaults，与外观模式同一条持久化路子 ——
    /// 用户切过一次，下次开 app 还停在那儿。默认层级（不动现有肌肉记忆）。
    @AppStorage(CrewSidebarViewMode.storageKey) private var viewModeRaw = CrewSidebarViewMode.default.rawValue

    private var viewMode: CrewSidebarViewMode { CrewSidebarViewMode.resolve(rawValue: viewModeRaw) }

    var body: some View {
        VStack(spacing: 0) {
            viewModePicker
            List {
                switch viewMode {
                case .hierarchy:
                    ForEach(machineGroups) { group in
                        Section {
                            sectionBody(group)
                        } header: {
                            machineHeader(group)
                        }
                    }
                case .timeline:
                    CrewTimelineListView(
                        crews: visibleCrews, childCrewTarget: $childCrewTarget)
                }
            }
            .listStyle(.sidebar)
            // 订阅额度（claude/codex 已用百分比 + 重置时刻）—— 不分登录态：
            // 本机 runner 的额度跟 edge 登录无关，本机模式同样要看得见。
            quotaFooterLine
            // 本机身份 + 今日用量。#63 之后不登录到任何地方,这一行只是展示。
            identityFooter
        }
        // 完全不设 navigationTitle —— 一旦设了（哪怕空串），SwiftUI 会不停把窗口
        // titleVisibility 设回 .visible，把我们在 WindowChromeConfigurator 里藏标题的
        // 设置顶掉，于是 App 名 "PendingCrew" 又冒回标题栏。不设它，标题由
        // WindowChromeConfigurator 一次性藏死。
        .toolbar {
            ToolbarItemGroup {
                Button {
                    showingCreateSheet = true
                } label: {
                    Label("新建 crew", systemImage: "plus")
                }
                .help("新建 crew")
                Button {
                    Task {
                        await crewStore.refreshList()
                        await crewStore.refreshMachines()
                    }
                } label: {
                    Label("刷新", systemImage: "arrow.clockwise")
                }
                .help("刷新 crew 列表")
                .disabled(crewStore.loadingList)
                Button {
                    showingWorkspaceSync = true
                } label: {
                    Label("Workspace 同步", systemImage: "arrow.triangle.2.circlepath")
                }
                .help("Workspace 同步")
            }
        }
        .sheet(isPresented: $showingCreateSheet) {
            CreateCrewSheet()
        }
        // 行右键进来的「在这下面建子 crew」—— 与 CrewDetailInspector 那条入口同一
        // 做法：传 parentCrewId，建完 CreateCrewSheet 自己 attachParent 挂到父之下。
        .sheet(item: $childCrewTarget) { target in
            CreateCrewSheet(parentCrewId: target.parentCrewId)
        }
        .sheet(isPresented: $showingWorkspaceSync) {
            WorkspaceSyncView()
        }
        // ④ 拦住：底下还有活跃子 crew 的父不许藏（理由当场说清）。挂在侧栏顶层
        // 而不是行上 —— 同 childCrewTarget 那个 sheet，行会被 List 回收。
        .alert("先顾一下底下那几个", isPresented: Binding(
            get: { crewStore.hideBlockedNotice != nil },
            set: { if !$0 { crewStore.hideBlockedNotice = nil } }
        )) {
            Button("好", role: .cancel) { crewStore.hideBlockedNotice = nil }
        } message: {
            Text(crewStore.hideBlockedNotice ?? "")
        }
        // 子 crew 全闲着 → 放行，但落地前说清楚会连着谁一起藏。
        .alert("连子 crew 一起藏起来", isPresented: Binding(
            get: { crewStore.pendingSubtreeHide != nil },
            set: { if !$0 { crewStore.pendingSubtreeHide = nil } }
        ), presenting: crewStore.pendingSubtreeHide) { pending in
            Button("藏起来") {
                crewStore.pendingSubtreeHide = nil
                Task { await crewStore.hideCrewFromUI(pending.crewId) }
            }
            Button("取消", role: .cancel) { crewStore.pendingSubtreeHide = nil }
        } message: { pending in
            Text("「\(pending.crewTitle)」底下挂着 \(pending.alsoHiddenCount) 个子 crew，"
                 + "它们会跟着一起从侧栏消失（都闲着，没有 session 在跑）。\n\n"
                 + "子 crew 不会被提到顶层显示 —— 那会让侧栏的组织架构和实际汇报线对不上。"
                 + "取回这个群时，它们一起回来。")
        }
    }

    // MARK: - 视图切换（层级 / 时间流）

    /// 侧栏顶部那条分段控件：一眼看得见、一下能切。放 List **之外**（不随列表
    /// 滚动、也不被 Section 头吃掉），两种视图切换时它自己不动。
    private var viewModePicker: some View {
        Picker("", selection: Binding(
            get: { viewMode },
            set: { viewModeRaw = $0.rawValue }
        )) {
            ForEach(CrewSidebarViewMode.allCases) { mode in
                Label(mode.label, systemImage: mode.systemImage)
                    .labelStyle(.titleAndIcon)
                    .tag(mode)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .controlSize(.small)
        .padding(.horizontal, 10)
        .padding(.top, 6)
        .padding(.bottom, 4)
        .help("层级：按机器 + 从属关系；时间流：拉平，最近有动静的排最上")
    }

    // MARK: - 机器分组

    private var machineGroups: [MachineGrouping.Group] {
        MachineGrouping.group(
            crews: visibleCrews,
            machines: crewStore.machines,
            localDeviceId: DeviceIdentity.current
        )
    }

    /// 侧栏该画出来的 crew（人手动藏起来的那些，连同跟着它们消失的子树，不在其中）。
    ///
    /// **层级视图和时间流视图喂的是同一份** —— 只改一种视图、另一种漏掉，是这类
    /// 改动最容易翻的车（时间流视图是 Todo #50 后加的）。判定在 `CrewHiding`。
    private var visibleCrews: [CrewSummary] { CrewHiding.visible(crewStore.crews) }

    @ViewBuilder
    private func sectionBody(_ group: MachineGrouping.Group) -> some View {
        if collapsedMachineIds.contains(group.id) {
            EmptyView()
        } else if group.crews.isEmpty {
            machineEmptyRow(group)
                .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
        } else {
            CrewDAGTreeView(crews: group.crews, childCrewTarget: $childCrewTarget)
                .listRowInsets(EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: 0))
        }
    }

    @ViewBuilder
    private func machineHeader(_ group: MachineGrouping.Group) -> some View {
        let machine = group.machine
        let isLocal = machine?.deviceId == DeviceIdentity.current
        let collapsed = collapsedMachineIds.contains(group.id)
        HStack(spacing: 6) {
            Button {
                if collapsed { collapsedMachineIds.remove(group.id) }
                else { collapsedMachineIds.insert(group.id) }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .rotationEffect(.degrees(collapsed ? 0 : 90))
                        .foregroundStyle(.secondary)
                    Image(systemName: machine?.displayIcon(isLocal: isLocal) ?? "questionmark.folder")
                        .foregroundStyle(.secondary)
                    Text(machine?.displayName ?? "其它")
                        .lineLimit(1)
                    if isLocal {
                        Text("本机")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(Capsule().fill(Theme.Palette.surfaceMuted))
                    }
                    if let m = machine {
                        Circle()
                            .fill(machineOnline(m) ? Color.green : Color.secondary.opacity(0.4))
                            .frame(width: 6, height: 6)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Spacer(minLength: 4)
            if isLocal {
                Button { showingCreateSheet = true } label: {
                    Image(systemName: "plus")
                }
                .buttonStyle(.plain)
                .help("在本机新建 crew")
            }
        }
    }

    private func machineOnline(_ m: Machine) -> Bool {
        m.deviceId == DeviceIdentity.current || m.status == "online"
    }

    @ViewBuilder
    private func machineEmptyRow(_ group: MachineGrouping.Group) -> some View {
        let isLocal = group.machine?.deviceId == DeviceIdentity.current
        VStack(alignment: .leading, spacing: 4) {
            if isLocal {
                if crewStore.loadingList {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("加载中…").foregroundStyle(.secondary)
                    }
                } else {
                    Text("还没有 crew").font(.callout).foregroundStyle(.secondary)
                    Text("点 + 新建第一个").font(.caption).foregroundStyle(.tertiary)
                }
            } else {
                // 其它机器：local-first,本机看不到其 crew 内容 → 遥控投影是后续(#242)。
                HStack(spacing: 6) {
                    Image(systemName: "antenna.radiowaves.left.and.right")
                        .foregroundStyle(.tertiary)
                    Text("遥控查看（稍后）").font(.caption).foregroundStyle(.tertiary)
                }
            }
        }
        .padding(.vertical, 6)
        .padding(.horizontal, 10)
    }

    /// Sidebar 底部固定条：「人」+ 随机头像 —— 与成员列表里那个「人」**同一张脸**
    /// (同一 seed `LocalWhiteboardStore.localUserId` 喂同一套 `BotAvatar`)，
    /// 底下跟今日 CC / Codex 用量小字行。
    ///
    /// #63:PendingCrew 不登录到任何地方,原来那条「点「人」开登录面板」的入口
    /// 和已登录态的 subject/签出菜单一并删掉 —— 这一行现在纯展示,不可点。
    /// 用量小字行原先嵌在已登录分支里,它跟登录无关(本机 runner 的 token 用量),
    /// 所以挪出来保住,不随登录层一起陪葬。
    private var identityFooter: some View {
        HStack(spacing: 10) {
            BotAvatar(seed: LocalWhiteboardStore.localUserId, size: 26)
                .frame(width: 26)
            VStack(alignment: .leading, spacing: 2) {
                Text("人")
                    .font(.callout.weight(.medium))
                agentUsageLine
            }
            Spacer(minLength: 4)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }
    /// 订阅额度显示（两家都拿不到时整块隐藏）。Todo #8/#10 把原来的文字表格
    /// 换成了圆环 —— 视图实现在 `QuotaRingsFooter`，判定在 `QuotaRingLayout`。
    private var quotaFooterLine: some View {
        QuotaRingsFooter(quota: quota)
    }

    /// 余量配色：已用 <60 绿(充裕) / <80 黄(过半) / <90 橙(将尽) / ≥90 红(告急)。
    /// **阈值的单一真值在 `QuotaLevel`（纯 Foundation，单测钉死）**，这里只做
    /// 档位 → 颜色的映射。
    static func quotaColor(level: QuotaLevel) -> Color {
        switch level {
        case .ample:    return .green
        case .half:     return .yellow
        case .near:     return .orange
        case .critical: return .red
        }
    }

    /// 今日 CC / Codex token 用量小字行（两项都 nil 时隐藏）。
    private var agentUsageLine: some View {
        AgentUsageLine(monitor: sessionHost.usage)
    }

}

/// 今日 CC / Codex token 用量小字行（两项都 nil 时隐藏）。
///
/// 单独成 View 是因为它读 `LocalAgentUsageMonitor` 的 `@Published`：monitor 现在
/// 由 app 级 `SessionHost` 持有（前后端分离 P0），侧栏从 `sessionHost.usage` 取到的
/// 计算属性拿不到刷新 —— 得有个 `@ObservedObject` 才会随它重绘。
private struct AgentUsageLine: View {
    @ObservedObject var monitor: LocalAgentUsageMonitor

    @ViewBuilder
    var body: some View {
        let cc = monitor.claudeTodayTokens
        let cx = monitor.codexTodayTokens
        if cc != nil || cx != nil {
            HStack(spacing: 6) {
                if let n = cc {
                    HStack(spacing: 2) {
                        Text("Claude")
                        Text(LocalAgentUsageMonitor.formatTokens(n))
                    }
                }
                if cc != nil, cx != nil { Text("·") }
                if let n = cx {
                    HStack(spacing: 2) {
                        Text("Codex")
                        Text(LocalAgentUsageMonitor.formatTokens(n))
                    }
                }
            }
            .font(.caption2)
            .foregroundStyle(.tertiary)
        }
    }
}

#endif
