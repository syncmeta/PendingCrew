#if os(macOS)
import AppKit
import SwiftUI

/// Todo 附件的两块小视图（Todo #52）：**已落盘**的缩略图条（面板行 / 详细窗口
/// 条目与追问共用）与**待发**的托盘（详细窗口里追问时用）。
///
/// 图走的是群聊那同一条渲染路：`ServerImage` → `CrewRemoteImage` 的 `file://`
/// 分支（后台解码 + 按显示尺寸降采样 + 缓存，见 #443），点开是群聊同一个
/// `ImageViewer`。非图给一个 chip，点了交系统打开。

/// 已落盘附件的横排缩略图。`compact` 给概览面板（小格子、不换行、超出省略）。
struct CrewTodoAttachmentStrip: View {
    let attachments: [LocalWhiteboardAttachment]
    /// 缩略图边长。概览面板 36、详细窗口 72。
    var cell: CGFloat = 72
    /// 最多显示几张（超出显示「+N」）。nil = 全显示。
    var maxVisible: Int? = nil

    @State private var zoomed: ZoomTarget?

    private var images: [LocalWhiteboardAttachment] { attachments.filter(\.isImage) }
    private var files: [LocalWhiteboardAttachment] { attachments.filter { !$0.isImage } }
    private var visibleImages: [LocalWhiteboardAttachment] {
        guard let maxVisible, images.count > maxVisible else { return images }
        return Array(images.prefix(maxVisible))
    }

    var body: some View {
        if attachments.isEmpty {
            EmptyView()
        } else {
            VStack(alignment: .leading, spacing: 6) {
                if !visibleImages.isEmpty {
                    HStack(spacing: 6) {
                        ForEach(visibleImages, id: \.id) { att in thumbnail(att) }
                        if images.count > visibleImages.count {
                            Text("+\(images.count - visibleImages.count)")
                                .font(Theme.Fonts.caption2)
                                .foregroundStyle(Theme.Palette.inkMuted)
                        }
                    }
                }
                ForEach(files, id: \.id) { att in fileChip(att) }
            }
            .sheet(item: $zoomed) { target in
                // 本地图不走鉴权，serverURL 只是签名占位（与群聊气泡同一处理）。
                ImageViewer(path: target.path, serverURL: URL(string: "file:///")!)
            }
        }
    }

    @ViewBuilder
    private func thumbnail(_ att: LocalWhiteboardAttachment) -> some View {
        let path = URL(fileURLWithPath: att.path).absoluteString
        ServerImage(path: path, serverURL: URL(string: "file:///")!,
                    contentMode: .fill, maxPixelSize: Int(cell * 3))
            .frame(width: cell, height: cell)
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(Rectangle())
            .onTapGesture { zoomed = ZoomTarget(path: path) }
            .help("点开看大图")
    }

    @ViewBuilder
    private func fileChip(_ att: LocalWhiteboardAttachment) -> some View {
        Button {
            NSWorkspace.shared.open(URL(fileURLWithPath: att.path))
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "doc.fill").font(Theme.Fonts.caption2)
                Text(att.filename ?? (att.path as NSString).lastPathComponent)
                    .font(Theme.Fonts.caption)
                    .lineLimit(1)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(Theme.Palette.surfaceMuted, in: Capsule())
        }
        .buttonStyle(.plain)
        .foregroundStyle(Theme.Palette.inkMuted)
        .help("用系统默认程序打开")
    }

    /// `.sheet(item:)` 要一个 Identifiable。
    private struct ZoomTarget: Identifiable {
        let path: String
        var id: String { path }
    }
}

/// 待发附件托盘（详细窗口追问时用）：缩略图 + 删除叉。与群聊 composer 托盘同一
/// 语义 —— 删掉一份就把它的暂存副本一起收掉，别在 temp 里留着几百 MB。
struct CrewTodoPendingAttachmentTray: View {
    @Binding var items: [PendingComposerAttachment]
    var cell: CGFloat = 44

    var body: some View {
        if items.isEmpty {
            EmptyView()
        } else {
            HStack(spacing: 6) {
                ForEach(items) { item in
                    ZStack(alignment: .topTrailing) {
                        preview(item)
                            .frame(width: cell, height: cell)
                            .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
                        Button {
                            remove(item)
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .font(.system(size: 12))
                                .foregroundStyle(.white, .black.opacity(0.55))
                        }
                        .buttonStyle(.plain)
                        .offset(x: 4, y: -4)
                        .help("不带这张了")
                    }
                }
            }
        }
    }

    @ViewBuilder
    private func preview(_ item: PendingComposerAttachment) -> some View {
        if let data = item.previewData, let image = NSImage(data: data) {
            Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
        } else {
            ZStack {
                Theme.Palette.surfaceMuted
                Image(systemName: "doc.fill")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.inkMuted)
            }
        }
    }

    private func remove(_ item: PendingComposerAttachment) {
        if let staged = item.stagedURL { CrewChatAttachmentStore.discardStaged(staged) }
        items.removeAll { $0.id == item.id }
    }
}
#endif
