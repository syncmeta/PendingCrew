#if os(macOS)
import SwiftUI

/// PendingCrew macOS 设置窗口（⌘,）。
///
/// 人类 2026-09-12（Todo #11）：「我希望设置里面专门有一个 tab 设置编码工具，特别是
/// 准备加 acp 的支持了。每一个 harness 都有一块设置的地方，不用点了才出来。要能更新、
/// 设置目录、检测。」—— 所以这里是分页的，编码工具单独一页，页面里每个 harness 一块，
/// 全部摊开，没有「点一下才出来」的框。
///
/// iPad shell 暂为占位页(task B2)，外观 Picker 届时在 iPad 设置入口再暴露。
struct CrewSettingsView: View {
    var body: some View {
        TabView {
            GeneralSettingsTab()
                .tabItem { Label("通用", systemImage: "gearshape") }
            CodingToolsSettingsTab()
                .tabItem { Label("编码工具", systemImage: "terminal") }
            BackendsSettingsTab()
                .tabItem { Label("后端", systemImage: "externaldrive.connected.to.line.below") }
        }
        .frame(width: 520, height: 520)
    }
}

/// 通用：外观 + 危险区。
private struct GeneralSettingsTab: View {
    @AppStorage(AppearanceMode.storageKey) private var appearanceRaw = AppearanceMode.default.rawValue
    @State private var showResetConfirm = false

    private var appearance: AppearanceMode {
        AppearanceMode(rawValue: appearanceRaw) ?? .default
    }

    var body: some View {
        Form {
            Section {
                Picker("外观", selection: Binding(
                    get: { appearance },
                    set: { appearanceRaw = $0.rawValue }
                )) {
                    ForEach(AppearanceMode.allCases) { mode in
                        Text(mode.label).tag(mode)
                    }
                }
                .pickerStyle(.segmented)
            } header: {
                Text("外观")
            }

            Section {
                Button(role: .destructive) {
                    showResetConfirm = true
                } label: {
                    Text("清除本机所有数据")
                        .foregroundStyle(.red)
                }
            } header: {
                Text("危险区")
            } footer: {
                Text("清除本机的设置和所有本地 crew 数据，清除后 app 将重启。")
            }
            .confirmationDialog(
                "清除本机所有数据?此操作不可恢复,仅清除本机数据。",
                isPresented: $showResetConfirm,
                titleVisibility: .visible
            ) {
                Button("清除本机所有数据", role: .destructive) {
                    LocalDataReset.performReset()
                }
                Button("取消", role: .cancel) {}
            }
        }
        .formStyle(.grouped)
        .padding()
    }
}

/// 编码工具：每个 harness 一块，摊开。
///
/// 人类 Todo #131 那条仍然成立 —— 版本只在设置里出现，不回侧栏页脚。
/// **这里是全 app 唯一用到 `AgentCLIVersionView` 的地方**（`ViewWiringTests` 的
/// 接线表钉着它，删了就红）。
private struct CodingToolsSettingsTab: View {
    @ObservedObject private var versions = AgentCLIVersionCenter.shared

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                ForEach([LocalCodingAgentKind.claudeCode, .codex], id: \.self) { kind in
                    GroupBox {
                        AgentCLIVersionView(center: versions, kind: kind)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(6)
                    }
                }
                Text("版本每 10 分钟自动检测一次，只读取本机版本，不判断是否最新；"
                     + "升级、回滚都要单独确认，且该 runner 必须没有存活 session。\n"
                     + "「CLI 所在目录」留空时按登录 shell 的 PATH 加一串常见安装位自动搜索；"
                     + "填了就只用它 —— 自动搜索找错了版本、或者装在冷僻位置时用这个。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .padding()
        }
        // 检测在打开设置时才起（`start()` 自带「只起一次」的门），关掉设置后那个
        // 10 分钟的轮询继续跑 —— 跟原来挂在侧栏页脚上时同一个共享中心。
        .task { versions.start() }
    }
}

/// 「管理、连接后端」（人类 Todo #11 的后半 / #121）。
///
/// 人类原话：「我希望 pendingcrew 要有管理后端的能力 **本机的后端也是一个** 要能管理
/// 这些的更新」。模型层（`BackendRegistry`）今天已经由别的 session 建好了，**而且
/// 一个文件都没引用它** —— 这一页就是把它接上。判定一条都不在这儿重写：能不能连、
/// 删不删得掉、读不出来怎么说，全问模型层。
private struct BackendsSettingsTab: View {
    @State private var load: BackendRegistry.Load = .fresh([])
    @State private var newName = ""
    @State private var newURL = ""
    /// 删除被拒 / 存盘失败时那句话。**拒绝必须看得见** —— 模型层特意为「内置那条
    /// 删不掉」写了一句解释，静默忽略等于把它扔了。
    @State private var notice: String?
    /// 重启入口要走 SessionHost（换代公告 / 停旧 / 问接回，与启动换代同一套）。
    @EnvironmentObject private var sessionHost: SessionHost
    /// 每条后端的实况。**判定全在 `BackendRegistry.liveStatus`** —— 远程和别处的 socket
    /// 它根本不探，这里也不许自己去探本机再填上去。
    @State private var statuses: [String: BackendLiveStatus] = [:]
    @State private var restartPrompt: RestartPrompt?
    @State private var restarting = false

    private struct RestartPrompt {
        var title: String
        var confirmation: String
    }

    /// viewer 里停掉后台，界面自己会马上拉一个新的 —— 按钮文案据此由模型层定。
    private var interfaceRelaunchesBackend: Bool { ProcessRole.effective == .viewer }

    /// 登记表跟锁、socket 一样落在数据根下（`PENDINGCREW_DATA_DIR` 挪走时跟着走）。
    private var registryFile: URL {
        BackendRegistry.registryFile
    }

    var body: some View {
        Form {
            if let problem = load.problem {
                Section {
                    Text(problem).foregroundStyle(.orange).font(.callout)
                } header: {
                    Text("读这份登记表时出了事")
                }
            }

            Section {
                ForEach(load.refs) { ref in
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 6) {
                            Text(ref.displayName).bold()
                            if ref.isBuiltIn {
                                Text("内置").font(.caption)
                                    .padding(.horizontal, 5).padding(.vertical, 1)
                                    .background(.quaternary, in: Capsule())
                            }
                        }
                        Text(address(of: ref)).font(.caption)
                            .foregroundStyle(.secondary).textSelection(.enabled)
                        // **能不能连由模型层说**，这里不自己判。远程那一档它会明确拒绝，
                        // 并且说明为什么不退回本机 —— 那句话要原样摆出来给人看。
                        if case let .unsupported(why) = BackendRegistry.connectivity(of: ref) {
                            Text(why).font(.caption).foregroundStyle(.orange)
                        }
                        if let status = statuses[ref.id] {
                            statusLine(status, isRemote: ref.isRemote)
                            restartButton(for: status)
                        }
                        Button(sessionHost.viewer?.selectedBackendID == ref.id
                               ? "当前后端" : "连接") {
                            notice = sessionHost.connectViewer(to: ref)
                        }
                        .disabled(sessionHost.viewer?.selectedBackendID == ref.id)
                    }
                    .padding(.vertical, 2)
                    .swipeActions {
                        // 内置那条也让划 —— **拒绝要由模型层说出来**，而不是这里
                        // 把按钮藏掉。藏掉的话人只会觉得「这条怎么没反应」。
                        Button(role: .destructive) { remove(ref) } label: { Text("移除") }
                    }
                }
                if let notice {
                    Text(notice).font(.caption).foregroundStyle(.orange)
                }
                Button("刷新实况") { refreshStatuses() }
                    .disabled(restarting)
            } header: {
                Text("认识的后端")
            } footer: {
                Text("本机那条是内置的：删不掉，也永远排第一 —— 删了之后这个界面就没有"
                     + "任何后端可连了。远程连接使用已配对的 TLS 安全通道；握手或信任"
                     + "失败会留在远程错误态，**不会**悄悄退回本机。")
            }
            Section {
                TextField("名字（你自己认得出就行）", text: $newName)
                    .textFieldStyle(.roundedBorder)
                TextField("地址，例如 pendingcrew+tls://host:7443", text: $newURL)
                    .textFieldStyle(.roundedBorder)
                Button("加进来") { add() }
                    .disabled(newName.trimmingCharacters(in: .whitespaces).isEmpty
                              || newURL.trimmingCharacters(in: .whitespaces).isEmpty)
            } header: {
                Text("加一个远程后端")
            } footer: {
                Text("**加进来不等于已经配对**。这一批要求信任账本中已有对应记录；"
                     + "Bonjour、二维码与完整配对界面留到下一批。")
            }
        }
        .formStyle(.grouped)
        .padding()
        .task { reload(); refreshStatuses() }
        .confirmationDialog(
            restartPrompt?.title ?? "",
            isPresented: Binding(get: { restartPrompt != nil },
                                 set: { if !$0 { restartPrompt = nil } }),
            titleVisibility: .visible
        ) {
            Button(restartPrompt?.title ?? "重启后台", role: .destructive) { restart() }
            Button("取消", role: .cancel) {}
        } message: {
            // 会打断几个、之后会不会问接回 —— 全是模型层给的原话。
            Text(restartPrompt?.confirmation ?? "")
        }
    }

    private func reload() { load = BackendRegistry.load(from: registryFile) }

    /// 探一遍实况。**本机那条要握手，主线程上最多顿 2 秒**（socket 回调投主队列）；
    /// 远程和别处的 socket 模型层根本不探。
    private func refreshStatuses() {
        var next: [String: BackendLiveStatus] = [:]
        for ref in load.refs { next[ref.id] = BackendRegistry.liveStatus(of: ref) }
        statuses = next
    }

    @ViewBuilder
    private func statusLine(_ status: BackendLiveStatus, isRemote: Bool) -> some View {
        switch status {
        case let .unsupported(why):
            // 远程那句上面 connectivity 已经原样摆出来了，别说两遍。
            if !isRemote { Text(why).font(.caption).foregroundStyle(.orange) }
        case .notRunning:
            Text("没有后台进程在跑。").font(.caption).foregroundStyle(.secondary)
        case let .undecidable(why):
            Text("有后台占着，但问不出它现在的情况：\(why)")
                .font(.caption).foregroundStyle(.orange)
        case let .running(build, pid, running, retained):
            VStack(alignment: .leading, spacing: 1) {
                Text("运行中 · 版本 \(build) · pid \(pid) · 在跑 \(running) 个 session"
                     + (retained > 0 ? "（另有 \(retained) 个已退出、画面还留着）" : ""))
                    .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                if build != SessionDaemonHost.currentBuild {
                    Text("和界面版本 \(SessionDaemonHost.currentBuild) 不一致。")
                        .font(.caption).foregroundStyle(.orange)
                }
            }
        }
    }

    /// 按钮能不能按、叫什么、确认框说什么，**全问 `BackendRegistry.restartAction`**。
    /// 不能按时不画按钮 —— 原因已经在实况那一行里说了。
    @ViewBuilder
    private func restartButton(for status: BackendLiveStatus) -> some View {
        if case let .available(title, confirmation) = BackendRegistry.restartAction(
            for: status, appBuild: SessionDaemonHost.currentBuild,
            interfaceRelaunchesBackend: interfaceRelaunchesBackend) {
            Button(restarting ? "正在处理…" : title) {
                restartPrompt = RestartPrompt(title: title, confirmation: confirmation)
            }
            .disabled(restarting)
        }
    }

    private func restart() {
        restartPrompt = nil
        restarting = true
        Task { @MainActor in
            notice = await sessionHost.restartLocalBackend()
            restarting = false
            // 新后台要一两秒才握得上手；立刻探会探到「没在跑」，吓人。
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            refreshStatuses()
        }
    }

    private func add() {
        let name = newName.trimmingCharacters(in: .whitespaces)
        let url = newURL.trimmingCharacters(in: .whitespaces)
        var refs = load.refs
        refs.append(BackendRef(id: UUID().uuidString.lowercased(),
                               displayName: name, transport: .remote(url: url)))
        persist(refs, onSuccess: { newName = ""; newURL = ""; notice = nil })
    }

    private func remove(_ ref: BackendRef) {
        switch BackendRegistry.removing(ref.id, from: load.refs) {
        case let .refused(why):
            notice = why          // 模型层那句解释原样摆出来，别自己另编一句
        case let .removed(rest):
            persist(rest, onSuccess: { notice = nil })
        }
    }

    /// 存盘。**失败必须说** —— 这本登记表跟白板在同一棵树下，那个周期性故障里它
    /// 一样写不进去；静默失败的话人会以为加上了，下次打开却不见。
    private func persist(_ refs: [BackendRef], onSuccess: () -> Void) {
        do {
            try FileManager.default.createDirectory(
                at: registryFile.deletingLastPathComponent(), withIntermediateDirectories: true)
            try BackendRegistry.save(refs, to: registryFile)
            onSuccess()
            reload()
        } catch {
            notice = "没写进去：\(error.localizedDescription)。这次的改动没有生效。"
        }
    }

    private func address(of ref: BackendRef) -> String {
        switch ref.transport {
        case let .localSocket(path): return path
        case let .remote(url): return url
        }
    }
}

#endif
