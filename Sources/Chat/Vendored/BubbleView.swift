// VENDORED from PendingBot apps/pendingbot/Sources/Features/Message/BubbleView.swift @ 43c8ea2e
// 再对齐 = 对照源文件重拷。仅允许的偏离打 `// PENDINGCREW SHIM:` 标注。
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif
// PENDINGCREW SHIM (Todo #3): NSWorkspace 打开本地 file:// 附件需要 AppKit。
#if canImport(AppKit)
import AppKit
#endif

/// Asymmetric layout inspired by Claude.ai (not iMessage):
///   • Bot — borderless, left-aligned, avatar + name row, then content.
///   • User — right-aligned, soft-tinted bubble with rounded radius.
///
/// Bot messages render full Markdown (headings, lists, fenced code with a
/// copy/run toolbar, tables, blockquotes) via `MarkdownText`. User input is
/// plain text — they just typed it, no need to interpret syntax.
struct BubbleView<Menu: View>: View {
    let message: ChatMessage
    let botName: String
    /// Conversation id — drives the per-conv pastel tint on BotAvatar.
    let conversationID: String
    /// Bot identity, passed in so the avatar emoji stays consistent for
    /// the same bot across all your conversations with it (background
    /// tint still varies per conv). nil falls back to conversationID
    /// for non-bot contexts.
    var botID: String? = nil
    /// Self-chat override: when set, the bot bubble uses the user's
    /// uploaded photo / BotAvatar(seed) instead of the standard
    /// per-bot glyph — the conversation reads as the user talking to
    /// a mirror of themselves.
    var selfAvatar: (seed: String, attachmentId: String?)? = nil
    /// Logged-in user's id, used to decide which side of the chat each
    /// bubble lives on. For user_user / group both parties share
    /// sender_type=="user"; without the id check both sides would
    /// render as right-aligned "mine" bubbles.
    var currentUserId: String? = nil
    /// Peer profile for user_user conversations — drives the left-side
    /// avatar on the other person's bubbles. nil for user_bot / self.
    /// `avatarSeed` is the server-supplied placeholder-emoji seed so the
    /// same person renders identically across every viewer's device.
    var peerProfile: (userId: String, displayName: String, avatarPath: String?, avatarSeed: String)? = nil
    /// Per-message sender resolution for *group* convs — many possible
    /// senders, name shown above the bubble. Takes precedence over
    /// peerProfile / selfAvatar / botID when set. `kind == "bot"`
    /// renders BotAvatar; `"user"` renders UserAvatar. The display
    /// name sits above the bubble so multi-sender threads stay
    /// readable.
    var groupSender: GroupBubbleSender? = nil
    let serverURL: URL?
    /// Web-search citations referenced by inline `[N]` markers in
    /// `message.content`. Empty for bubbles that didn't trigger search.
    var citations: [MessageCitation] = []
    /// Whether this bubble is the live streaming target — drives the
    /// trailing cursor. Caller clears it the moment the reveal finishes
    /// so the cursor's `.opacity` transition fades it out.
    var isStreaming: Bool = false
    /// True while we're streaming but the reveal buffer is empty (no new
    /// token yet). Switches the cursor from solid to TTY-style blink.
    var streamPaused: Bool = false
    /// Tapping the failure indicator on a failed outgoing message
    /// re-sends it. nil leaves the indicator non-interactive.
    var onRetry: (() -> Void)? = nil
    /// Group convs — right-click (macOS) / long-press (iOS) the sender
    /// avatar to @-mention them into the composer. Passed the resolved
    /// `groupSender`. nil (1:1 / non-group) = no avatar mention menu.
    var onMentionSender: ((GroupBubbleSender) -> Void)? = nil
    /// Long-press context-menu items. Attached to just the avatar+bubble
    /// region (see `botLayout` / `userLayout`) — NOT the full-width row —
    /// so the system highlight platter hugs the bubble instead of
    /// spanning the whole row width.
    @ViewBuilder var menu: () -> Menu

    private var hasText: Bool { !message.content.isEmpty }

    /// User bubble background — switches on the optimistic-send state:
    ///   • failed   → red wash
    ///   • sending  → lighter green (request in flight, no HTTP ack yet)
    ///   • sent / nil → standard green (server returned 200 or canonical row)
    private var userBubbleFill: Color {
        if message.isFailed { return Color.red.opacity(0.12) }
        if message.isSending { return Theme.Palette.userBubbleSending }
        return Theme.Palette.userBubble
    }

    var body: some View {
        if message.isMine(currentUserId: currentUserId) {
            userLayout
        } else {
            botLayout
        }
    }

    // ── Bot ─────────────────────────────────────────────────────────────────

    private var botLayout: some View {
        HStack(alignment: .top, spacing: 0) {
            HStack(alignment: .top, spacing: 8) {
            Group {
                if let g = groupSender {
                    // PENDINGCREW SHIM: crew messages use CrewAvatarBadges (captain star /
                    // session status dot / terminal-icon) instead of bare BotAvatar/UserAvatar.
                    CrewAvatarBadges(sender: g, size: 30)
                } else if let peer = peerProfile, message.sender_type == "user" {
                    // user_user / group peer — use UserAvatar so it matches
                    // the same person's avatar in the friends list.
                    UserAvatar(seed: peer.avatarSeed, attachmentId: peer.avatarPath, size: 30)
                } else if let s = selfAvatar {
                    UserAvatar(seed: s.seed, attachmentId: s.attachmentId, size: 30)
                } else {
                    BotAvatar(
                        emojiSeed: (botID?.isEmpty == false ? botID! : conversationID),
                        colorSeed: conversationID,
                        size: 30
                    )
                }
            }
            .padding(.top, 2)
            .modifier(AvatarMentionMenu(sender: groupSender, action: onMentionSender))

            VStack(alignment: .leading, spacing: 4) {
                if let g = groupSender {
                    Text(g.displayName)
                        .font(Theme.Fonts.rounded(size: 11, weight: .semibold))
                        .foregroundStyle(Theme.Palette.inkMuted)
                }

                if let attachments = message.attachments, !attachments.isEmpty,
                   let serverURL {
                    AttachmentGrid(attachments: attachments, serverURL: serverURL)
                }

                if hasText || isStreaming {
                    MarkdownText(text: message.content,
                                 allowCodeRun: true,
                                 citations: citations)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .background(
                            RoundedRectangle(cornerRadius: Theme.Metrics.bubbleRadius,
                                             style: .continuous)
                                .fill(Theme.Palette.surface)
                        )
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.Metrics.bubbleRadius,
                                             style: .continuous)
                                .strokeBorder(Theme.Palette.hairline, lineWidth: 0.5)
                        )
                        // Trailing caret — sits at the bottom-trailing of
                        // the rendered text. Inline-with-last-char would
                        // be nicer but isn't reachable through MarkdownUI;
                        // the bubble's trailing edge still reads as
                        // "more on the way".
                        .overlay(alignment: .bottomTrailing) {
                            if isStreaming {
                                BlinkingCursor(paused: streamPaused)
                                    .padding(.trailing, 12)
                                    .padding(.bottom, 12)
                                    .transition(.opacity)
                            }
                        }
                        .animation(.easeOut(duration: 0.28), value: isStreaming)
                }
            }
            }
            .contextMenu { menu() }

            Spacer(minLength: 32)
        }
        .padding(.horizontal, Theme.Metrics.gutter)
        .padding(.vertical, 4)
    }

    // ── User ────────────────────────────────────────────────────────────────

    private var userLayout: some View {
        HStack(alignment: .top) {
            Spacer(minLength: 48)

            VStack(alignment: .trailing, spacing: 6) {
                if let attachments = message.attachments, !attachments.isEmpty,
                   let serverURL {
                    AttachmentGrid(attachments: attachments, serverURL: serverURL,
                                   alignment: .trailing)
                }

                if hasText {
                    HStack(alignment: .center, spacing: 6) {
                        if message.isFailed {
                            Button {
                                onRetry?()
                            } label: {
                                Image(systemName: "exclamationmark.circle.fill")
                                    .font(Theme.Fonts.glyph(size: 18))
                                    .foregroundStyle(Color.red.opacity(0.85))
                            }
                            .buttonStyle(.plain)
                            .disabled(onRetry == nil)
                            .accessibilityLabel("重新发送")
                        }
                        Text(message.content)
                            // PENDINGCREW SHIM (#443): 常开 → 由 `crewBubbleSelectable`
                            // 决定。见 CrewBubbleTextSelection.swift 顶部的 hang 栈。
                            .crewSelectableText()
                            .font(Theme.Fonts.body)
                            .foregroundStyle(message.isFailed
                                             ? Color.red.opacity(0.95)
                                             : Theme.Palette.ink)
                            .padding(.horizontal, 14)
                            .padding(.vertical, 10)
                            .background(
                                RoundedRectangle(cornerRadius: Theme.Metrics.bubbleRadius,
                                                 style: .continuous)
                                    .fill(userBubbleFill)
                            )
                            .overlay(
                                RoundedRectangle(cornerRadius: Theme.Metrics.bubbleRadius,
                                                 style: .continuous)
                                    .strokeBorder(
                                        message.isFailed ? Color.red.opacity(0.55) : .clear,
                                        lineWidth: 1
                                    )
                            )
                            .animation(.easeInOut(duration: 0.22), value: message.status)
                    }
                }
            }
            .contextMenu { menu() }
        }
        .padding(.horizontal, Theme.Metrics.gutter)
        .padding(.vertical, 4)
    }
}

// PENDINGCREW SHIM: GroupBubbleSender is defined in Sources/Chat/Adapter/GroupBubbleSender.swift
// (extracted so the type compiles in Foundation-only test bundles without SwiftUI).
// BubbleView references it by name; both files land in the same module.

struct AttachmentGrid: View {
    let attachments: [Attachment]
    let serverURL: URL
    /// Side the bubble sits on — own messages pass `.trailing` so the
    /// image grid hugs the right edge like the text bubble; peers leave
    /// the `.leading` default.
    var alignment: HorizontalAlignment = .leading

    @State private var zoomed: ZoomTarget?

    private var images: [Attachment] { attachments.filter(\.isImage) }
    private var files: [Attachment] { attachments.filter { !$0.isImage } }
    /// Fixed thumbnail edge. Fixed-size cells keep the grid intrinsically
    /// sized so the parent VStack's alignment can position it — a
    /// `.flexible()` grid would stretch full-width and float the image.
    private let cell: CGFloat = 110

    /// Images chunked into rows of up to 3.
    private var imageRows: [[Attachment]] {
        stride(from: 0, to: images.count, by: 3).map {
            Array(images[$0 ..< min($0 + 3, images.count)])
        }
    }

    var body: some View {
        VStack(alignment: alignment, spacing: 6) {
            ForEach(imageRows.indices, id: \.self) { row in
                HStack(spacing: 6) {
                    ForEach(imageRows[row]) { att in
                        // PENDINGCREW SHIM: 本地图按格子尺寸降采样解码（#443）——
                        // 110pt 的缩略图不需要 4000px 原图，×3 给 retina + 放大留余量。
                        ServerImage(path: att.url, serverURL: serverURL,
                                    contentMode: .fill,
                                    maxPixelSize: Int(cell * 3))
                            .frame(width: cell, height: cell)
                            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                            .contentShape(Rectangle())
                            .onTapGesture { zoomed = ZoomTarget(path: att.url) }
                    }
                }
            }
            ForEach(files) { att in
                FileAttachmentChip(attachment: att)
            }
        }
        .platformFullScreenCover(item: $zoomed) { target in
            ImageViewer(path: target.path, serverURL: serverURL)
        }
    }

    // PENDINGCREW SHIM: iOS 的「保存到相册」原本经 auth-gated `/v1/uploads/:id`
    // 把整图下下来再写相册。那条通道随 #63 第二期删除跨端遥控整层一起没了
    // （附件此后只有本地落盘的 `file://` 一种来源，而 iOS 上根本没有本地后端），
    // 所以这个长按菜单一并去掉，而不是留一个必然失败的按钮。
}

/// Identifiable wrapper so a tapped image path can drive `.fullScreenCover`.
private struct ZoomTarget: Identifiable {
    let path: String
    var id: String { path }
}

/// A non-image attachment rendered as an icon + filename chip. Tapping it hands
/// the local file to the system (Finder on macOS / share sheet on iOS).
struct FileAttachmentChip: View {
    let attachment: Attachment

    @State private var shareURL: IdentifiedURL?
    @State private var loadError = false


    private var displayName: String { attachment.filename ?? "文件" }

    private var sizeText: String? {
        guard let size = attachment.size, size > 0 else { return nil }
        return ByteCountFormatter.string(fromByteCount: Int64(size), countStyle: .file)
    }

    var body: some View {
        Button {
            Task { await openFile() }
        } label: {
            HStack(spacing: 10) {
                Image(systemName: iconName)
                    .font(Theme.Fonts.glyph(size: 22))
                    .foregroundStyle(Theme.Palette.accent)
                    .frame(width: 30)
                VStack(alignment: .leading, spacing: 2) {
                    Text(displayName)
                        .font(Theme.Fonts.system(size: 14, weight: .medium))
                        .foregroundStyle(Theme.Palette.ink)
                        .lineLimit(2)
                        .truncationMode(.middle)
                    if let sizeText {
                        Text(sizeText)
                            .font(Theme.Fonts.system(size: 11))
                            .foregroundStyle(Theme.Palette.inkMuted)
                    }
                }
                Spacer(minLength: 4)
                Image(systemName: loadError ? "exclamationmark.triangle" : "square.and.arrow.up")
                    .font(Theme.Fonts.glyph(size: 13))
                    .foregroundStyle(Theme.Palette.inkMuted)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .frame(maxWidth: 260, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(Theme.Palette.surface)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(Theme.Palette.hairline, lineWidth: 0.5)
            )
        }
        .buttonStyle(.plain)
        // System share sheet (UIActivityViewController) is iOS-only; on
        // macOS the file is still downloaded to a temp dir but the share
        // presentation is skipped until a macOS share path lands.
        #if os(iOS)
        .sheet(item: $shareURL) { wrapped in
            ShareSheet(items: [wrapped.url])
        }
        #endif
    }

    /// SF Symbol picked from the MIME so the chip reads at a glance.
    private var iconName: String {
        let mime = attachment.mime.lowercased()
        if mime == "application/pdf" { return "doc.richtext" }
        if mime.hasPrefix("audio/") { return "waveform" }
        if mime.hasPrefix("video/") { return "film" }
        if mime.hasPrefix("text/") { return "doc.text" }
        if mime.contains("zip") || mime.contains("compressed") || mime.contains("archive") {
            return "doc.zipper"
        }
        return "doc.fill"
    }

    private func openFile() async {
        // PENDINGCREW SHIM (Todo #3)：附件只有本地落盘（`file://` 绝对 URL）一种
        // 来源 —— edge 的 auth-gated `/v1/uploads/:id` 下载随 #63 第二期删掉了。
        // 拿不到 file:// 就如实报错，不再留一条必然 401 的网络路径。
        loadError = false
        guard attachment.url.hasPrefix("file://"),
              let url = URL(string: attachment.url) else {
            loadError = true
            Haptics.error()
            return
        }
        #if os(macOS)
        NSWorkspace.shared.open(url)
        #else
        shareURL = IdentifiedURL(url: url)
        #endif
    }
}

/// Attaches a "@ <name>" context menu to the sender avatar so右键(macOS)/长按
/// (iOS)可把该发送者 @ 进 composer。仅当是群消息(`sender != nil`)且宿主给了
/// `action`(群聊上下文)时挂载——1:1 / 无 action 时原样返回,头像不弹菜单。
/// 嵌套在气泡外层 contextMenu 之内:SwiftUI 命中最内层,头像区弹 @ 菜单、其余
/// 区域仍是气泡自己的菜单(回复/复制…)。
private struct AvatarMentionMenu: ViewModifier {
    let sender: GroupBubbleSender?
    let action: ((GroupBubbleSender) -> Void)?
    func body(content: Content) -> some View {
        if let sender, let action {
            content.contextMenu {
                Button {
                    action(sender)
                } label: {
                    Label("@ \(sender.displayName)", systemImage: "at")
                }
            }
        } else {
            content
        }
    }
}

/// `.sheet(item:)` wants an Identifiable payload — a thin wrapper so a
/// freshly downloaded file URL can drive the share sheet presentation.
struct IdentifiedURL: Identifiable {
    let url: URL
    var id: String { url.absoluteString }
}

/// Minimal UIActivityViewController bridge for the file share sheet.
/// iOS-only — wraps UIKit's UIActivityViewController.
#if os(iOS)
struct ShareSheet: UIViewControllerRepresentable {
    let items: [Any]

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
#endif
