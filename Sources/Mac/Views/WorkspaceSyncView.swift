#if os(macOS)
import SwiftUI

/// Workspace 同步执行页——Task 8「给引擎套 SwiftUI 壳」的落地:顶部方向切换 +
/// 「扫描」「上行同步」「拉取收敛」三个按钮 + WIP commit Toggle,下面是
/// plan item / receipt 混合列表(同一 itemId,扫描后是 plan item 行,执行后
/// 换成一条或多条 receipt 行——`executeUp` 一个项目可能产出 push receipt +
/// 单独的回写失败 receipt,两条都要如实看见,不合并)。
///
/// 未配置 workspace root 时只显引导占位——设置入口(选 root 目录 / 填 remote)
/// 是 Task 9 的 sheet,这里只放文案 + 禁用按钮,不接线。
struct WorkspaceSyncView: View {
    /// 方向类型直接用 `WorkspaceSyncStore.Direction`(不再本地重复定义一份同名
    /// enum)——store 的 `planDirection` guard 与这里的 Picker 选中值天然共用
    /// 同一套 case,不存在"两份 enum 靠人工保持同步"的漂移风险。`label` 是纯
    /// UI 文案,挂成 view-local 扩展。
    private typealias Direction = WorkspaceSyncStore.Direction

    @StateObject private var store = WorkspaceSyncStore()
    @State private var direction: Direction = .up
    @State private var wipCommit = false
    @State private var showingSetupSheet = false

    private var busy: Bool {
        store.phase == .scanning || store.phase == .syncing
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            if store.isConfigured {
                content
            } else {
                unconfiguredPlaceholder
            }
        }
        .frame(minWidth: 420, minHeight: 480)
        // 切方向必须清空当前 plan——否则「Picker 已经切到下行,列表却还挂着
        // 上行扫描结果」这种视觉污染会误导用户以为下行也扫过了。这是第一道
        // 防线;store 的 `planDirection` guard 是第二道(即便这里漏调,
        // `runUp`/`runDown` 也不会真的跑错方向)。
        .onChange(of: direction) { _, _ in
            store.resetPlan()
        }
        .sheet(isPresented: $showingSetupSheet) {
            WorkspaceSetupSheet(store: store)
        }
    }

    // MARK: - 头部

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Workspace 同步")
                .font(Theme.Fonts.headline)
                .foregroundStyle(Theme.Palette.ink)

            Picker("方向", selection: $direction) {
                ForEach(Direction.allCases) { d in
                    Text(d.label).tag(d)
                }
            }
            .pickerStyle(.segmented)
            .disabled(!store.isConfigured || busy)
            .frame(maxWidth: 220)

            HStack(spacing: 10) {
                Button {
                    switch direction {
                    case .up: store.scanUp()
                    case .down: store.scanDown()
                    }
                } label: {
                    Label("扫描", systemImage: "magnifyingglass")
                }
                .disabled(!store.isConfigured || busy)

                Button {
                    store.runUp(wipCommit: wipCommit)
                } label: {
                    Label("上行同步", systemImage: "arrow.up.circle")
                }
                .disabled(!store.isConfigured || busy || direction != .up
                          || !store.planItems.contains { $0.actionable })

                Button {
                    store.runDown()
                } label: {
                    Label("拉取收敛", systemImage: "arrow.down.circle")
                }
                .disabled(!store.isConfigured || busy || direction != .down
                          || !store.planItems.contains { $0.actionable })

                Spacer()

                Toggle("WIP commit", isOn: $wipCommit)
                    .toggleStyle(.switch)
                    .disabled(!store.isConfigured || busy)
                    .help("上行时若某项目有未提交变更,是否允许引擎自动打一个 WIP commit 再推。")
            }

            if busy {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text(store.phase == .scanning ? "扫描中…" : "同步中…")
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.inkMuted)
                }
            }

            if case .done(let ok, let failed) = store.phase {
                resultBar(ok: ok, failed: failed)
            }

            if let error = store.lastError {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.danger)
                    .textSelection(.enabled)
            }
        }
        .padding(14)
    }

    @ViewBuilder
    private func resultBar(ok: Int, failed: Int) -> some View {
        HStack(spacing: 6) {
            Image(systemName: failed > 0 ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                .foregroundStyle(failed > 0 ? Theme.Palette.amber : Theme.Palette.success)
            Text("\(ok) 成功 / \(failed) 失败")
                .font(Theme.Fonts.footnote.weight(.medium))
                .foregroundStyle(Theme.Palette.ink)
        }
    }

    // MARK: - 未配置占位

    // 设置入口(选 root 目录 / 填 remote)——Task 9:「打开设置」拉起
    // `WorkspaceSetupSheet`,配置成功后 sheet 自动关、`store.isConfigured`
    // 翻真,body 据此切到 `content`。
    private var unconfiguredPlaceholder: some View {
        VStack(spacing: 10) {
            Image(systemName: "shippingbox")
                .font(.system(size: 32))
                .foregroundStyle(Theme.Palette.inkMuted)
            Text("尚未配置 workspace 仓库")
                .font(Theme.Fonts.subheadline.weight(.medium))
                .foregroundStyle(Theme.Palette.ink)
            Text("创建一个新的,或绑定已有的 workspace 仓库地址。")
                .font(Theme.Fonts.caption)
                .foregroundStyle(Theme.Palette.inkMuted)
            Button {
                showingSetupSheet = true
            } label: {
                Label("打开设置", systemImage: "gearshape")
            }
            .padding(.top, 4)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - 列表

    private var content: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 6) {
                if let workspaceReceipt = store.workspaceReceipt {
                    receiptRow(workspaceReceipt, title: "Workspace 仓库(拉取刷新)")
                }
                if store.planItems.isEmpty && store.workspaceReceipt == nil {
                    Text("还没有扫描结果 —— 点「扫描」看看现在有什么需要同步。")
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.inkMuted)
                        .padding(.horizontal, 14)
                        .padding(.top, 8)
                } else {
                    ForEach(store.planItems) { item in
                        let matched = store.receipts.filter { $0.itemId == item.id }
                        if matched.isEmpty {
                            planItemRow(item)
                        } else {
                            ForEach(Array(matched.enumerated()), id: \.offset) { _, receipt in
                                receiptRow(receipt, title: item.title)
                            }
                        }
                    }
                }
            }
            .padding(.horizontal, 10)
            .padding(.bottom, 12)
        }
    }

    @ViewBuilder
    private func planItemRow(_ item: SyncPlanItem) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: item.kind == .workspaceRepo ? "shippingbox" : "folder")
                .foregroundStyle(Theme.Palette.inkMuted)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title)
                    .font(Theme.Fonts.footnote.weight(.medium))
                    .foregroundStyle(item.actionable ? Theme.Palette.ink : Theme.Palette.inkMuted)
                if !item.detail.isEmpty {
                    Text(item.detail)
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.inkMuted)
                }
            }
            Spacer(minLength: 0)
        }
        .opacity(item.actionable ? 1 : 0.6)
        .padding(.vertical, 6)
        .padding(.horizontal, 8)
        .background(Theme.Palette.surfaceMuted.opacity(0.4), in: RoundedRectangle(cornerRadius: 8))
    }

    @ViewBuilder
    private func receiptRow(_ receipt: SyncReceipt, title: String) -> some View {
        let (icon, color) = Self.iconAndColor(for: receipt.outcome)
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: icon)
                .foregroundStyle(color)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(Theme.Fonts.footnote.weight(.medium))
                    .foregroundStyle(Theme.Palette.ink)
                Text(Self.receiptDetailText(receipt))
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .textSelection(.enabled)
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 6)
        .padding(.horizontal, 8)
        .background(Theme.Palette.surfaceMuted.opacity(0.4), in: RoundedRectangle(cornerRadius: 8))
    }

    // MARK: - Receipt 展示助手

    private static func iconAndColor(for outcome: Outcome) -> (String, Color) {
        switch outcome {
        case .uploaded, .upToDate, .pulled:
            return ("checkmark.circle.fill", Theme.Palette.success)
        case .skipped:
            return ("exclamationmark.triangle.fill", Theme.Palette.amber)
        case .failed:
            return ("xmark.circle.fill", Theme.Palette.danger)
        }
    }

    private static func outcomeSummary(_ outcome: Outcome) -> String {
        switch outcome {
        case .uploaded(let remoteHead):
            return "已上传 → \(shortHash(remoteHead))"
        case .upToDate:
            return "已是最新"
        case .pulled(let newHead):
            return "已拉取 → \(shortHash(newHead))"
        case .skipped(let reason):
            return "跳过：\(reason)"
        case .failed(let reason):
            return "失败：\(reason)"
        }
    }

    private static func receiptDetailText(_ receipt: SyncReceipt) -> String {
        var parts = [outcomeSummary(receipt.outcome), formattedTime(receipt.at)]
        if let detail = receipt.detail, !detail.isEmpty {
            parts.append(detail)
        }
        return parts.joined(separator: " · ")
    }

    private static func shortHash(_ hash: String) -> String {
        String(hash.prefix(8))
    }

    private static func formattedTime(_ iso: String) -> String {
        guard let date = ISO8601DateFormatter().date(from: iso) else { return iso }
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter.string(from: date)
    }
}

/// 纯 UI 文案——`WorkspaceSyncStore.Direction` 本身是给 store 的执行方向 guard
/// 用的领域枚举,不该带中文标签;这里单独扩展一份仅供本 view 的 Picker 使用。
private extension WorkspaceSyncStore.Direction {
    var label: String {
        switch self {
        case .up: return "上行"
        case .down: return "下行"
        }
    }
}
#endif
