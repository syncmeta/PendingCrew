#if os(macOS)
import SwiftUI

/// PendingCrew macOS 设置窗口（⌘,）。
///
/// 外观 Picker 三态（跟随系统/浅色/深色）；编码工具版本（自动检测）；其余只保留
/// 本机数据管理。
///
/// iPad shell 暂为占位页(task B2)，外观 Picker 届时在 iPad 设置入口再暴露。
struct CrewSettingsView: View {
    @AppStorage(AppearanceMode.storageKey) private var appearanceRaw = AppearanceMode.default.rawValue
    @State private var showResetConfirm = false
    /// 本机 claude / codex 的 CLI 版本（人类 Todo #131：从侧栏左下角挪到这里）。
    @ObservedObject private var versions = AgentCLIVersionCenter.shared

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

            // 人类 Todo #131：版本原先挂在侧栏左下角的额度环那一行右侧，人类要它
            // 只在设置里出现。**这里是全 app 唯一的调用点**（`ViewWiringTests` 的
            // 接线表钉着它，删了就红）。
            //
            // 形态跟订阅档位（Todo #87）对齐：**只自动检测、不给人工填**。所以这块
            // 没有输入框，只有一行「检测到的版本」，维护动作藏在它的 popover 里、
            // 逐次确认才执行。
            Section {
                ForEach([LocalCodingAgentKind.claudeCode, .codex], id: \.self) { kind in
                    LabeledContent(kind.displayName) {
                        AgentCLIVersionView(center: versions, kind: kind)
                    }
                }
            } header: {
                Text("编码工具版本")
            } footer: {
                Text("自动检测本机 claude / codex 的版本，不用填。点版本号可以升级、"
                     + "回滚到本机保留的旧版、运行健康检查；每次执行都要单独确认，"
                     + "且该 runner 必须没有存活 session。")
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
        .frame(width: 420)
        .padding()
        // 检测在打开设置时才起（`start()` 自带「只起一次」的门），关掉设置后那个
        // 10 分钟的轮询继续跑 —— 跟原来挂在侧栏页脚上时同一个共享中心。
        .task { versions.start() }
    }
}
#endif
