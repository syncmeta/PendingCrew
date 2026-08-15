#if os(macOS)
import SwiftUI

/// 左栏：crew 列表 + toolbar（新建 / 刷新）。
///
/// MVP 阶段是平铺 list —— DAG 树形 sidebar 留下次（spec v2 §12.1 说明）。
/// `runtimeLocation` 用一个 systemImage + 缩写标签提示用户这是 local /
/// peer / fly。
struct CrewSidebarView: View {
    @EnvironmentObject private var crewStore: CrewStore
    @EnvironmentObject private var model: AppModel
    @State private var showingCreateSheet = false
    /// 行右键「在这下面建子 crew」选中的父 crew（非 nil = 开子 crew 表单）。
    /// sheet 挂在侧栏顶层而不是行上：List 行会被回收，挂行上的表单可能被顶掉。
    @State private var childCrewTarget: CrewChildCreationTarget?
    @State private var showingWorkspaceSync = false
    @State private var confirmingSignOut = false
    /// 本机模式下侧栏的可选登录入口（登录 SSO C2）—— 弹直接登录 sheet（CrewLoginSheet）。
    @State private var showingLogin = false
    /// 折叠起来的机器分组 id 集合（默认全展开）。
    @State private var collapsedMachineIds: Set<String> = []
    /// 登录账号身份（头像 seed + 显示名兜底）—— 在 footer `.task` 里从家族凭据
    /// (共享 keychain)解析一次,不在 render 路径上碰 keychain。见 `identityFooter`。
    @State private var identityAvatarSeed: String?
    /// 家族凭据里的真实显示名 —— subjects 还没拉回来(或拉取失败)时的标题兜底,
    /// 避免 footer 显示"已登录"占位。generic 凭据(无 displayName)不算。
    @State private var credentialDisplayName: String?
    @StateObject private var usageMonitor = LocalAgentUsageMonitor()
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
                        crews: crewStore.crews, childCrewTarget: $childCrewTarget)
                }
            }
            .listStyle(.sidebar)
            // 订阅额度（claude/codex 已用百分比 + 重置时刻）—— 不分登录态：
            // 本机 runner 的额度跟 edge 登录无关，本机模式同样要看得见。
            quotaFooterLine
            // 当前身份 + 切换/登出。device grant 是 subject-scoped(spec v2 §4.4),
            // 想换 subject 必须撤掉这台机器的 grant 重新登录 —— 所以"切换身份"
            // 路径就是签出后再去直接登录页登一遍。这里集中放在 sidebar 底部,
            // 让 user 始终能看见"我现在以谁的身份在花钱"。
            identityFooter
        }
        // 额度中心 + 可用模型表中心一起常开（都是幂等启动、都要落文件给 helper 读）。
        .task { quota.start(); ModelCatalogCenter.shared.start() }
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
        // 只有已登录的 footer 菜单能触发(未登录 footer 是登录入口)——
        // 接合 v2 后没有"退出本机模式"概念,本地 crew 永远在。
        .confirmationDialog(
            "签出当前身份",
            isPresented: $confirmingSignOut,
            titleVisibility: .visible
        ) {
            Button("签出", role: .destructive) {
                signOut()
            }
            Button("取消", role: .cancel) {}
        } message: {
            Text("签出后本机 device grant 立即失效;若需切换到别的身份(本人 / 群账号),签出后重新扫码即可。本机 crew 不受影响。")
        }
        // 本机模式可选登录（登录 SSO C2）：先展示确认卡（有本机凭据时）
        // 或直接登录页（无凭据时）。登录成功后 CrewLoginSheet 自动 dismiss。
        .sheet(isPresented: $showingLogin) {
            CrewLoginSheet()
                .environmentObject(model)
                .frame(minWidth: 420, minHeight: 480)
        }
        .onAppear { usageMonitor.start() }
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
            crews: crewStore.crews,
            machines: crewStore.machines,
            localDeviceId: DeviceIdentity.current
        )
    }

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

    /// Sidebar 底部固定条。
    /// - 已登录:显示代表的 subject + ⋯ 菜单(签出/切换身份)。
    /// - 未登录:显示「人」+ 随机头像 —— 与成员列表里那个「人」**同一张脸**
    ///   (同一 seed `LocalWhiteboardStore.localUserId` 喂同一套 `BotAvatar`)。
    ///   不再写「登录」二字;登录入口收进这一行的点击 —— 点「人」照样开登录
    ///   面板(面板里再展示 PendingBot 直登或扫码登录),登录后本行变成账号。
    @ViewBuilder
    private var identityFooter: some View {
        Group {
            if model.isAuthenticated {
                HStack(spacing: 10) {
                    // 登录账号头像:用家族凭据里的 avatar_seed(与登录确认卡 BotAvatar
                    // 同字形,跨 app 一致);缺凭据(如未签名构建)回落 subject id —— 仍每
                    // 账号稳定。原先这里是写死的 SF Symbol 占位,不随身份变(`identityIcon`
                    // 已退役)。
                    BotAvatar(
                        seed: identityAvatarSeed ?? crewStore.subjects.first?.id ?? "?",
                        size: 26
                    )
                    .frame(width: 26)
                    VStack(alignment: .leading, spacing: 2) {
                        // Todo #55:账号类型跟在名字**后面同一行**(原来占 VStack 第二行),
                        // 让页脚从三行收到两行。侧栏很窄,所以挤不下时先截名字不截标识
                        // —— 标识 `.fixedSize()` 钉住自己的理想宽,名字 lineLimit(1)
                        // 吃剩下的空间并优先分配(layoutPriority)。
                        HStack(spacing: 5) {
                            Text(identityTitle)
                                .font(.callout.weight(.medium))
                                .lineLimit(1)
                                .layoutPriority(1)
                            // 账号类型只在 subject 解析出来后显示 —— 未解析时不渲染,
                            // 避免标题/副标题双双回落成"已登录"的重复占位。
                            if let subtitle = identitySubtitle {
                                Text(subtitle)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                                    .fixedSize()
                            }
                        }
                        agentUsageLine
                    }
                    Spacer(minLength: 4)
                    Menu {
                        Button {
                            confirmingSignOut = true
                        } label: {
                            Label("签出 / 切换身份", systemImage: "rectangle.portrait.and.arrow.right")
                        }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                            .imageScale(.medium)
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .fixedSize()
                }
                // 身份解析放 `.task`(不在 render 路径碰 keychain);subject 解析
                // 出来后 id 变 → 重跑一次拿到稳定回落。家族凭据里的 avatar_seed 与登录
                // 确认卡同字形,缺则回落 subject id;displayName 兜标题(generic 不算)。
                // subjects 还空着(首拉失败/未跑)就补拉一次 —— 拉成功后 id 变会
                // 自然重跑本 task,不会循环。
                .task(id: crewStore.subjects.first?.id) {
                    let cred = CrewLoginIdentity.current()
                    credentialDisplayName = (cred?.isGeneric == false) ? cred?.title : nil
                    identityAvatarSeed = cred?.avatarSeed ?? crewStore.subjects.first?.id
                    if crewStore.subjects.isEmpty {
                        await crewStore.refreshSubjects()
                    }
                }
            } else {
                Button {
                    showingLogin = true
                } label: {
                    HStack(spacing: 10) {
                        // 与成员列表里的「人」同源:seed 都是 localUserId,走同一个
                        // BotAvatar → 两处必然同一张脸。
                        BotAvatar(seed: LocalWhiteboardStore.localUserId, size: 26)
                            .frame(width: 26)
                        Text("人")
                            .font(.callout.weight(.medium))
                        Spacer(minLength: 4)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("点一下登录 PendingBot 账号")
            }
        }
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
    @ViewBuilder
    private var agentUsageLine: some View {
        let cc = usageMonitor.claudeTodayTokens
        let cx = usageMonitor.codexTodayTokens
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

    // identity 三件套只在已登录 footer 渲染(未登录 footer 是登录菜单,
    // 自带文案)—— 接合 v2 后不再按 mode 分叉。头像走 `BotAvatar(seed:)`
    // (见 footer),不再用 SF Symbol。
    // 标题优先级:edge 真实 subject → 家族凭据 displayName → "已登录"最终兜底。
    private var identityTitle: String {
        crewStore.subjects.first?.displayName ?? credentialDisplayName ?? "已登录"
    }

    /// 账号类型副标题;subject 未解析时返回 nil(footer 隐藏该行,不再重复"已登录")。
    private var identitySubtitle: String? {
        guard let kind = crewStore.subjects.first?.kindEnum else { return nil }
        return kind == .groupAccount ? "群账号" : "本人账号"
    }

    private func signOut() {
        // 接合 v2:签出只是撤掉登录能力叠加,本机 crew / 本机模式不受影响。
        guard model.isAuthenticated else { return }
        model.clearAuth()
        // crewStore.reset() 由 RootView 监听登录态变化自动调,
        // 不在这里重复 wire 以避免双触发顺序歧义。
    }
}

#endif
