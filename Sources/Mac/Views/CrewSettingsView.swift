#if os(macOS)
import SwiftUI

/// PendingCrew macOS 设置窗口（⌘,）。
///
/// 外观 Picker 对齐 PendingBot SettingsView —— 三态(跟随系统/浅色/深色)，
/// `@AppStorage(AppearanceMode.storageKey)` 写盘，`PendingCrewApp.body` 的
/// `.preferredColorScheme` 实时响应。
///
/// iPad shell 暂为占位页(task B2)，外观 Picker 届时在 iPad 设置入口再暴露。
struct CrewSettingsView: View {
    @AppStorage(AppearanceMode.storageKey) private var appearanceRaw = AppearanceMode.default.rawValue
    @AppStorage(AgentSubscriptionPlanPreference.claudeKey) private var claudePlanRaw = ""
    @AppStorage(AgentSubscriptionPlanPreference.codexKey) private var codexPlanRaw = ""
    @ObservedObject private var quota = QuotaCenter.shared
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
            } footer: {
                Text("「跟随系统」随设备的浅色/深色自动切换；也可固定为浅色或深色。")
            }

            Section {
                subscriptionPicker(
                    "Claude Code", selection: $claudePlanRaw,
                    choices: AgentSubscriptionPlanPreference.claudeChoices)
                subscriptionPicker(
                    "Codex", selection: $codexPlanRaw,
                    choices: AgentSubscriptionPlanPreference.codexChoices)
            } header: {
                Text("我的订阅档位")
            } footer: {
                Text("自动检测：Claude \(quota.claude?.subscriptionPlan ?? "未探到")；Codex \(quota.codex?.subscriptionPlan ?? "未探到")。人工选择只补档位标签，不推算或编造 token / requests 绝对额度。")
            }
            .onChange(of: claudePlanRaw) { _, _ in
                quota.subscriptionPlanPreferencesDidChange()
            }
            .onChange(of: codexPlanRaw) { _, _ in
                quota.subscriptionPlanPreferencesDidChange()
            }

            UpdateSettingsSection()

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
        // 比原来的 420 宽 —— 更新区要列提交标题(本仓库的标题动辄几十字),
        // 420 下每条都折成三四行,读不了。
        .frame(width: 560)
        .padding()
    }

    @ViewBuilder
    private func subscriptionPicker(
        _ title: String, selection: Binding<String>, choices: [String]
    ) -> some View {
        Picker(title, selection: selection) {
            ForEach(choices, id: \.self) { choice in
                Text(choice.isEmpty ? "自动检测" : choice).tag(choice)
            }
        }
    }
}
#endif
