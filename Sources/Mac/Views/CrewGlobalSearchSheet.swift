#if os(macOS)
import SwiftUI

/// 本机全部 crew 的事实搜索入口。读取白板 JSON 放在后台线程，结果匹配复用
/// `CrewMessageSearch`；隐藏群也在范围内，因为「从侧栏隐藏」不等于删除历史。
struct CrewGlobalSearchSheet: View {
    @EnvironmentObject private var crewStore: CrewStore
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var documents: [CrewMessageSearchDocument] = []
    @State private var loading = true

    private struct Scope: Sendable {
        let id: String
        let title: String
    }

    private var trimmedQuery: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var results: [CrewMessageSearchResult] {
        CrewMessageSearch.search(
            documents, query: trimmedQuery,
            limit: CrewMessageSearch.maximumLimit, order: .newestFirst)
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("正文、发送者、附件名或时间", text: $query)
                    .textFieldStyle(.plain)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .background(Theme.Palette.surfaceMuted)
            Divider()

            if loading {
                ProgressView("正在读取群聊索引…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if trimmedQuery.isEmpty {
                placeholder(icon: "magnifyingglass", text: "搜索所有群的消息")
            } else if results.isEmpty {
                placeholder(icon: "magnifyingglass", text: "没有找到匹配消息")
            } else {
                List(results) { result in
                    resultRow(result)
                }
                .listStyle(.inset)
            }

            Divider()
            HStack {
                Text("范围：全部 \(crewStore.crews.count) 个群；最多显示最新 200 条")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(.secondary)
                Spacer()
                Button("完成") { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
            .padding(12)
        }
        .frame(minWidth: 620, minHeight: 500)
        .navigationTitle("搜索所有群")
        .task { await loadDocuments() }
    }

    private func resultRow(_ result: CrewMessageSearchResult) -> some View {
        Button {
            crewStore.openChatSearchResult(result, query: trimmedQuery)
            dismiss()
        } label: {
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 6) {
                    Text(result.document.crewTitle)
                        .font(Theme.Fonts.headline)
                    Text(result.document.senderName)
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(.secondary)
                    Spacer()
                    if result.matchedFields.contains(.attachment) {
                        Image(systemName: "paperclip")
                            .foregroundStyle(.secondary)
                            .accessibilityLabel("附件元数据命中")
                    }
                    Text(result.document.createdAt)
                        .font(Theme.Fonts.caption2.monospacedDigit())
                        .foregroundStyle(.tertiary)
                }
                Text(result.document.text.isEmpty ? "（仅附件）" : result.document.text)
                    .lineLimit(2)
                    .foregroundStyle(.primary)
                if result.matchedFields.contains(.attachment) {
                    Text(result.document.attachmentMetadata.joined(separator: " · "))
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Text("群 \(result.document.crewTitle) · 消息 \(result.document.messageId.prefix(8))")
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(.tertiary)
            }
            .padding(.vertical, 5)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func placeholder(icon: String, text: String) -> some View {
        VStack(spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 40))
                .foregroundStyle(.tertiary)
            Text(text).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func loadDocuments() async {
        let scopes = crewStore.crews.map { Scope(id: $0.id, title: $0.title) }
        documents = await Task.detached(priority: .userInitiated) {
            scopes.flatMap { scope in
                LocalWhiteboardStore.shared.list(crewId: scope.id).map {
                    CrewMessageSearchAdapters.local(
                        $0, crewId: scope.id, crewTitle: scope.title)
                }
            }
        }.value
        loading = false
    }
}
#endif
