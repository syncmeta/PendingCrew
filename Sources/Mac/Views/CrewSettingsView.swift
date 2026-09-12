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

    /// 登记表跟锁、socket 一样落在数据根下（`PENDINGCREW_DATA_DIR` 挪走时跟着走）。
    private var registryFile: URL {
        PendingCrewDataRoot.subdirectory("backends").appendingPathComponent("registry.json")
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
                    }
                    .padding(.vertical, 2)
                }
            } header: {
                Text("认识的后端")
            } footer: {
                Text("本机那条是内置的：删不掉，也永远排第一 —— 删了之后这个界面就没有"
                     + "任何后端可连了。远程那一档还没做，列在这里只是为了让你看见"
                     + "「它还没做」，连不上时**不会**悄悄退回本机。")
            }
        }
        .formStyle(.grouped)
        .padding()
        .task { load = BackendRegistry.load(from: registryFile) }
    }

    private func address(of ref: BackendRef) -> String {
        switch ref.transport {
        case let .localSocket(path): return path
        case let .remote(url): return url
        }
    }
}

#endif
