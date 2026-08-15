#if os(macOS)
import SwiftUI

/// Workspace 首次设置 sheet(Task 9)——`WorkspaceSyncView` 未配置占位的
/// 「打开设置」入口:填本地路径 + 可选 remote URL,点「创建/绑定」调
/// `store.setup(root:remoteURL:)`。
///
/// - 本地路径默认 `~/PendingCrew/workspace`(spec §10 待用户决策项,先用这个
///   默认,可改——见 brief)。
/// - remote URL 可留空 = 先纯本地,之后可再配(`WorkspaceRepoService.ensure`
///   在 `remoteURL == nil` 时走 scaffold 分支)。
/// - 表单惯例照 `CreateCrewSheet`:header / form / footer 三段,`.plain`
///   按钮 + `.borderedProminent` 主操作,错误用 `.red` caption 如实显示。
/// - `store` 由父 view(`WorkspaceSyncView`)传入同一个实例(不是新建一个)
///   ——setup 成功后父 view 的 `isConfigured` 得跟着翻,同一个 `@Published`
///   源头才能让父子两处 view 观察到同一次状态变化。
struct WorkspaceSetupSheet: View {
    @ObservedObject var store: WorkspaceSyncStore
    @Environment(\.dismiss) private var dismiss

    @State private var rootPath: String = Self.defaultRootPath
    @State private var remoteURLText: String = ""
    @State private var busy = false

    private static var defaultRootPath: String {
        "~/PendingCrew/workspace"
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            form
            Divider()
            footer
        }
        .frame(width: 480)
        .frame(minHeight: 280)
    }

    // MARK: - sections

    private var header: some View {
        HStack {
            Text("设置 Workspace 仓库")
                .font(.title3.weight(.semibold))
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    private var form: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                rootField
                remoteField
                if let error = store.lastError {
                    Label(error, systemImage: "exclamationmark.triangle.fill")
                        .font(.caption)
                        .foregroundStyle(.red)
                        .textSelection(.enabled)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(20)
        }
    }

    private var rootField: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("本地路径")
                .font(.callout.weight(.medium))
            TextField("~/PendingCrew/workspace", text: $rootPath)
                .textFieldStyle(.roundedBorder)
                .disabled(busy)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text("workspace 仓库落地的目录——已存在的话直接使用,不存在会新建。")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var remoteField: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("远端 URL(可选)")
                .font(.callout.weight(.medium))
            TextField("留空 = 先纯本地,之后可再配", text: $remoteURLText)
                .textFieldStyle(.roundedBorder)
                .disabled(busy)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text("已有仓库就填它的地址(会 clone);第一次建议留空,先本地跑起来。")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var footer: some View {
        HStack(spacing: 8) {
            Spacer()
            Button("取消") { dismiss() }
                .keyboardShortcut(.cancelAction)
                .disabled(busy)
            Button {
                submit()
            } label: {
                if busy {
                    ProgressView().controlSize(.small)
                } else {
                    Text("创建/绑定")
                }
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.defaultAction)
            .disabled(busy || trimmedRootPath.isEmpty)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    // MARK: - actions

    private var trimmedRootPath: String {
        rootPath.trimmingCharacters(in: .whitespaces)
    }

    private func submit() {
        let expanded = (trimmedRootPath as NSString).expandingTildeInPath
        let root = URL(fileURLWithPath: expanded)
        let trimmedRemote = remoteURLText.trimmingCharacters(in: .whitespaces)
        let remoteURL: String? = trimmedRemote.isEmpty ? nil : trimmedRemote

        busy = true
        store.setup(root: root, remoteURL: remoteURL) {
            busy = false
            if store.lastError == nil {
                dismiss()
            }
        }
    }
}
#endif
