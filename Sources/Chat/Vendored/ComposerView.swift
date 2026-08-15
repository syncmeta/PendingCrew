// VENDORED from PendingBot apps/pendingbot/Sources/Features/Message/ComposerView.swift @ 43c8ea2e
// 再对齐 = 对照源文件重拷。仅允许偏离打 `// PENDINGCREW SHIM:` 标注。
import SwiftUI
import PhotosUI
#if os(iOS)
import UIKit
#elseif os(macOS)
import AppKit
#endif

/// Bottom-of-screen input bar — capsule field flanked by circle buttons.
/// Matches the PendingBot composer:
///   • Left  — circle attach button (hairline border, surface fill)
///   • Center — capsule textfield (1–6 lines, hairline border)
///   • Right — circle send button (state-aware: accent on, muted off)
///
/// The "+" panel is rendered inline below the input row. The host mounts
/// this whole bar inside a UIKit `inputAccessoryView` (see
/// ChatComposerAccessory.swift) so it tracks the keyboard frame-by-frame
/// during an interactive scroll-to-dismiss instead of lagging on SwiftUI's
/// keyboard-avoidance curve.
struct ComposerView: View {
    @Binding var input: String
    @Binding var pending: [PendingAttachment]
    @Binding var photoItems: [PhotosPickerItem]
    @Binding var cameraImage: PlatformImage?
    /// Flipped true by the "文件" action tile; the host owns the
    /// `.fileImporter` sheet bound to it.
    @Binding var showFileImporter: Bool
    /// Flipped true by the "图片" / "拍照" action tiles. The host owns the
    /// `.photosPicker` / camera `.fullScreenCover` bound to these. Presenting
    /// from the main view tree (not from inside the inputAccessoryView's
    /// hosting controller) is what keeps those pickers reliable — same pattern
    /// as `showFileImporter`.
    @Binding var showPhotoPicker: Bool
    @Binding var showCamera: Bool
    var canSend: Bool
    var onSend: () -> Void
    /// True while a bot reply is streaming. Send button morphs into a stop
    /// button — tapping it calls `onStop` which cancels the SSE turn task.
    var isStreaming: Bool = false
    var onStop: () -> Void = {}
    /// Optional manual lookback trigger. ConversationView passes a closure
    /// when the conv has a single bot (user_bot / self); group + user_user
    /// pass nil and the action sheet hides the tile.
    var onLookback: (() -> Void)? = nil
    /// When false, the "+" attachment button and its action panel are hidden
    /// entirely. Crew chat (T4.6 P1) has no attachments, so it passes false
    /// rather than surfacing a "+" that opens tiles wired to dead pickers.
    var showsAttachments: Bool = true
    /// #242 遥控 v1 — optional「派任务」entry. Non-nil renders a terminal
    /// glyph button in the leading accessory slot (where "+" sits for
    /// attachment-enabled hosts); crew chat passes a closure that opens the
    /// task-dispatch sheet. Default nil → no button (all other hosts).
    var onTask: (() -> Void)? = nil
    // PENDINGCREW SHIM: custom placeholder so PendingCrew's crew chat can
    // surface "发消息…（不 @ = 广播给所有 session）" without forking the view.
    var placeholder: String = "发消息…"
    // PENDINGCREW SHIM (task #487): crew chat 的 Todo 切换按钮。非 nil 时在
    // "+" 右边渲一个 checklist 圆钮，点亮（true = accent 底白字）后发送 = 创建
    // Todo（宿主 send 路径分流）。PendingBot 宿主不传 → 不渲染，行为不变。
    var todoMode: Binding<Bool>? = nil

    // PENDINGCREW SHIM (Todo #3): ⌘V 粘贴拦截。宿主返回 true = 剪贴板内容
    // （图片字节 / 文件 URL）已被收进附件托盘，吞掉本次 paste；false = 走默认
    // 文本粘贴。PendingBot 宿主不传 → 行为不变。目前只在 macOS 文本框生效。
    var onPasteAttachments: (() -> Bool)? = nil

    // PENDINGCREW SHIM (task #487 / Todo #1b): placeholder 随发送态切换。发消息态
    // 用宿主传入的 `placeholder`（crew chat = 「发群消息」），Todo 钮点亮后切成
    // 「新建 Todo」，读作「当前发送 = 记一条 Todo」。宿主不传 todoMode → 恒为
    // 静态 placeholder，行为不变。
    private var effectivePlaceholder: String {
        if let todoMode, todoMode.wrappedValue { return "新建 Todo" }
        return placeholder
    }

    @State private var showActions = false
    // Plain Bool, not @FocusState: the actual first responder is the
    // UIKit UITextView inside ComposerTextField, and nothing here is
    // bound via `.focused()`. SwiftUI's focus engine would reset a
    // @FocusState back to false on every parent re-render (which fires
    // on every keystroke, since `input` is host @State), causing
    // updateUIView to resign the text view — keyboard drops per char.
    @State private var isFocused: Bool = false

    var body: some View {
        VStack(spacing: 0) {
            if !pending.isEmpty {
                pendingTray
            }
            HStack(alignment: .bottom, spacing: 8) {
                if showsAttachments {
                Button {
                    Haptics.tap()
                    // Scope the action-sheet animation to a withAnimation
                    // block so it doesn't bleed into the keyboard's frame
                    // change. The keyboard goes via UIKit's resignFirstResponder
                    // / becomeFirstResponder path and animates with the system's
                    // native curve; if isFocused were toggled inside withAnimation
                    // it'd retime to easeInOut(0.22) and look like a slow fall.
                    if showActions {
                        withAnimation(.easeInOut(duration: 0.22)) {
                            showActions = false
                        }
                        isFocused = true
                    } else {
                        isFocused = false
                        withAnimation(.easeInOut(duration: 0.22)) {
                            showActions = true
                        }
                    }
                } label: {
                    Image(systemName: showActions ? "xmark" : "plus")
                        .font(Theme.Fonts.glyph(size: 18, weight: .medium))
                        .foregroundStyle(Theme.Palette.inkMuted)
                        .frame(width: 38, height: 38)
                        .floatingComposerSurface(shape: Circle())
                }
                .buttonStyle(.plain)
                }

                // PENDINGCREW SHIM (task #487): Todo 切换钮 —— 点亮态复用发送钮的
                // accent 圆底白字语言，读作「当前发送 = 创建 Todo」。
                if let todoMode {
                    Button {
                        Haptics.tap()
                        todoMode.wrappedValue.toggle()
                    } label: {
                        Image(systemName: "checklist")
                            .font(Theme.Fonts.glyph(size: 16, weight: .medium))
                            .foregroundStyle(todoMode.wrappedValue ? .white : Theme.Palette.inkMuted)
                            .frame(width: 38, height: 38)
                            .background {
                                if todoMode.wrappedValue { Circle().fill(Theme.Palette.accent) }
                            }
                            .floatingComposerSurface(shape: Circle())
                            .animation(.easeInOut(duration: 0.18), value: todoMode.wrappedValue)
                    }
                    .buttonStyle(.plain)
                    .help("Todo 模式：点亮后发送 = 记一条 Todo")
                }

                if let onTask {
                    Button {
                        Haptics.tap()
                        onTask()
                    } label: {
                        Image(systemName: "terminal")
                            .font(Theme.Fonts.glyph(size: 17, weight: .medium))
                            .foregroundStyle(Theme.Palette.inkMuted)
                            .frame(width: 38, height: 38)
                            .floatingComposerSurface(shape: Circle())
                    }
                    .buttonStyle(.plain)
                }

                HStack(alignment: .bottom, spacing: 6) {
                    ComposerTextField(
                        text: $input,
                        placeholder: effectivePlaceholder, // PENDINGCREW SHIM: 随 todoMode 动态切换
                        isFocused: $isFocused,
                        onHardwareReturn: {
                            // Hardware keyboard's plain Return — Mac Catalyst
                            // and iPad-with-keyboard. Shift+Return falls
                            // through to default UITextView behaviour (newline)
                            // by way of the subclass not consuming it.
                            if canSend { onSend() }
                        },
                        onPasteAttachments: onPasteAttachments // PENDINGCREW SHIM (Todo #3)
                    )
                    .onChange(of: isFocused) { _, focused in
                        if focused {
                            withAnimation(.easeInOut(duration: 0.22)) {
                                showActions = false
                            }
                        }
                    }
                    .padding(.leading, 14)
                    .padding(.vertical, 4)

                    Button(action: { isStreaming ? onStop() : onSend() }) {
                        Image(systemName: isStreaming ? "stop.fill" : "arrow.up")
                            .font(Theme.Fonts.glyph(size: isStreaming ? 12 : 15, weight: .bold))
                            // Subtle SF Symbol "fly up & replace" when the
                            // arrow morphs into the stop glyph (and back) —
                            // the old symbol lifts up and out, the new one
                            // rises in. Reads as "sent" without being flashy.
                            // Needs the stable accessory host (see
                            // ChatComposerAccessory.swift) to actually fire;
                            // a re-created/AnyView root would skip it.
                            .contentTransition(.symbolEffect(.replace.upUp))
                            .foregroundStyle(
                                isStreaming || canSend
                                    ? .white
                                    : Theme.Palette.inkMuted.opacity(0.5)
                            )
                            .frame(width: 30, height: 30)
                            .background(
                                Circle().fill(
                                    isStreaming || canSend
                                        ? Theme.Palette.accent
                                        : Theme.Palette.surfaceMuted
                                )
                            )
                            // Short ease on color so the gray↔accent flip
                            // when canSend toggles isn't a hard cut. Kept
                            // brief so it never feels laggy after the user
                            // types the first character.
                            .animation(.easeInOut(duration: 0.18), value: canSend)
                            .animation(.easeInOut(duration: 0.18), value: isStreaming)
                    }
                    .buttonStyle(.plain)
                    .disabled(!isStreaming && !canSend)
                    .padding(.trailing, 6)
                    .padding(.bottom, 4)
                }
                .floatingComposerSurface(
                    shape: RoundedRectangle(cornerRadius: 22, style: .continuous)
                )
            }
            .padding(.horizontal, Theme.Metrics.gutter)
            .padding(.top, 8)
            // Visual clearance between the capsule and the bar's bottom
            // edge. The home-indicator / keyboard-top safe-area gap is added
            // by the inputAccessoryView container, so this is purely the
            // breathing room above that.
            .padding(.bottom, 12)

            if showActions && showsAttachments {
                ChatActionSheet(
                    onPickPhoto: { showPhotoPicker = true },
                    onPickCamera: { showCamera = true },
                    onPickFile: { showFileImporter = true },
                    onDismiss: { showActions = false },
                    onLookback: onLookback
                )
                .frame(height: 240)
                .background(Theme.Palette.canvas)
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        // No SwiftUI background here in either state. Both backstops live in
        // the accessory container (ComposerBarContainerView): the progressive
        // blur when docked, a solid canvas when the "+" panel is open. They're
        // painted in UIKit so they reach the home-indicator strip — a SwiftUI
        // `.background` can't, since the hosted content sizes to itself above
        // the strip and the accessory's hosting controller never hands SwiftUI
        // the bottom safe area, so `.ignoresSafeArea(.bottom)` gets no frame
        // down there. The panel's own tiles (ChatActionSheet) and the pending
        // tray still carry their own opaque canvas fills.
        // No outer `.animation(..., value: showActions)` modifier — the
        // showActions toggles are wrapped in withAnimation at their call
        // sites instead, so the keyboard's frame change (driven by
        // resign/becomeFirstResponder, not by showActions) is not retimed
        // to SwiftUI's curve and keeps UIKit's native open/close cadence.
    }

    // ── Attachment thumbnails strip ────────────────────────────────────────

    private var pendingTray: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(pending) { att in
                    ZStack(alignment: .topTrailing) {
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(Theme.Palette.surfaceMuted)
                            .frame(width: 64, height: 64)
                            .overlay(pendingCellContent(att))
                            .overlay(pendingUploadOverlay(att))
                            .overlay(
                                RoundedRectangle(cornerRadius: 10, style: .continuous)
                                    .strokeBorder(Theme.Palette.hairline, lineWidth: 0.5)
                            )
                        Button {
                            pending.removeAll { $0.id == att.id }
                            Haptics.tap()
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .font(Theme.Fonts.body)
                                .foregroundStyle(.white, .black.opacity(0.55))
                        }
                        // Without .plain, macOS gives this its default bordered
                        // chrome — a shadowed rounded slab around the glyph.
                        .buttonStyle(.plain)
                        .offset(x: 6, y: -6)
                    }
                }
            }
            .padding(.horizontal, Theme.Metrics.gutter)
            .padding(.top, 8)
        }
        .background(Theme.Palette.canvas)
    }

    /// 64×64 thumbnail content for one pending attachment — the actual
    /// image for images, an icon + truncated filename for files.
    @ViewBuilder
    private func pendingCellContent(_ att: PendingAttachment) -> some View {
        if att.isImage {
            if let data = att.localPreviewData, let image = PlatformImage.decode(data) {
                Image(platformImage: image)
                    .resizable()
                    .scaledToFill()
                    .frame(width: 64, height: 64)
                    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            } else if let remoteId = att.uploadedAttachmentId {
                ServerImage(
                    path: "/v1/uploads/\(remoteId)",
                    serverURL: URL(string: "https://placeholder.invalid")!, // PENDINGCREW SHIM: HostedConfig not available; ServerImage shim ignores serverURL
                    contentMode: .fill
                )
                .frame(width: 64, height: 64)
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            } else {
                Image(systemName: "photo")
                    .font(Theme.Fonts.glyph(size: 22))
                    .foregroundStyle(Theme.Palette.inkMuted)
            }
        } else {
            VStack(spacing: 3) {
                Image(systemName: "doc.fill")
                    .font(Theme.Fonts.glyph(size: 20))
                    .foregroundStyle(Theme.Palette.accent)
                Text(att.filename ?? "文件")
                    .font(Theme.Fonts.system(size: 9))
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
                    .truncationMode(.middle)
            }
            .padding(.horizontal, 4)
        }
    }

    @ViewBuilder
    private func pendingUploadOverlay(_ att: PendingAttachment) -> some View {
        switch att.uploadState {
        case .uploaded:
            EmptyView()
        case .uploading:
            ZStack {
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(.black.opacity(0.35))
                VStack(spacing: 5) {
                    ProgressView()
                        .controlSize(.small)
                        .tint(.white)
                    Text("上传中")
                        .font(Theme.Fonts.system(size: 9, weight: .medium))
                        .foregroundStyle(.white)
                }
            }
        case .failed:
            ZStack {
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(.black.opacity(0.48))
                VStack(spacing: 4) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(Theme.Fonts.glyph(size: 16, weight: .semibold))
                        .foregroundStyle(.white)
                    Text("上传失败")
                        .font(Theme.Fonts.system(size: 9, weight: .semibold))
                        .foregroundStyle(.white)
                }
            }
        }
    }
}

// ── Floating surface backing ───────────────────────────────────────────────

/// Pure-white pill / circle with a barely-there drop shadow, used as the
/// background for both the "+" button and the input capsule. On iOS 26 we
/// upgrade to Liquid Glass via `.glassEffect`, which adds depth-aware
/// refraction on top of the same white tint and softer shadow.
private extension View {
    @ViewBuilder
    func floatingComposerSurface<S: InsettableShape>(shape: S) -> some View {
        if #available(iOS 26.0, macOS 26.0, *) {
            self
                .background(shape.fill(Theme.Palette.surface))
                .glassEffect(.regular, in: shape)
                .shadow(color: Color.black.opacity(0.06), radius: 6, x: 0, y: 1)
        } else {
            self
                .background(shape.fill(Theme.Palette.surface))
                .overlay(shape.strokeBorder(Color.black.opacity(0.04), lineWidth: 0.5))
                .shadow(color: Color.black.opacity(0.08), radius: 6, x: 0, y: 2)
        }
    }
}

// ── Progressive blur backdrop ──────────────────────────────────────────────

/// Soft top-to-bottom blur sitting behind the floating composer. Messages
/// scrolling underneath fade from fully visible at the top edge of the
/// inset region to fully blurred down past the home indicator. A single
/// `UIVisualEffectView` with a `CAGradientLayer` mask: the gradient runs
/// from clear → opaque downwards, so the blur layer is invisible at the
/// top and fully revealed at the bottom.
///
/// Earlier draft stacked 4 layered blurs of increasing strength to fake a
/// variable-radius Gaussian, but the visual outcome wasn't worth the
/// complexity — the single masked blur reads as a clean reveal here.
///
/// Used directly (as a UIView) by ComposerBarContainerView, which pins it to
/// the full accessory bounds so it covers the home-indicator strip — see
/// ChatComposerAccessory.swift.
#if os(iOS)
final class ProgressiveBlurBackdropView: UIView {
    private let blur = UIVisualEffectView(effect: UIBlurEffect(style: .systemUltraThinMaterial))
    private let gradient = CAGradientLayer()

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        addSubview(blur)
        blur.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            blur.topAnchor.constraint(equalTo: topAnchor),
            blur.bottomAnchor.constraint(equalTo: bottomAnchor),
            blur.leadingAnchor.constraint(equalTo: leadingAnchor),
            blur.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
        // Multi-stop gradient gives a softer ramp than a 0→1 linear; the
        // 0.0/0.35/0.7/1.0 stops with eased alpha hide the seam where the
        // mask first reveals the blur layer.
        gradient.colors = [
            UIColor.black.withAlphaComponent(0).cgColor,
            UIColor.black.withAlphaComponent(0.15).cgColor,
            UIColor.black.withAlphaComponent(0.55).cgColor,
            UIColor.black.withAlphaComponent(1).cgColor,
        ]
        gradient.locations = [0.0, 0.35, 0.7, 1.0]
        gradient.startPoint = CGPoint(x: 0.5, y: 0)
        gradient.endPoint = CGPoint(x: 0.5, y: 1)
        blur.layer.mask = gradient
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    override func layoutSubviews() {
        super.layoutSubviews()
        // Disable implicit CALayer animation — the gradient mask should
        // snap to new bounds during keyboard/inset frame changes, not
        // animate independently from UIKit's transition.
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        gradient.frame = blur.bounds
        CATransaction.commit()
    }
}

// ── Composer text input ────────────────────────────────────────────────────

/// UITextView-backed input that grows up to 6 lines, distinguishes
/// hardware-Return-as-send from Shift+Return-as-newline, and
/// preserves newlines on paste. SwiftUI's `TextField(axis: .vertical)`
/// can do none of those: it can't see the modifier flags on a
/// keypress, and its `onChange` fires for every newline regardless of
/// source (paste vs typed), which made any paste containing a
/// linebreak fire send immediately.
///
/// Behavior summary:
/// - Soft keyboard Return → newline (no auto-send).
/// - Hardware Return (no modifiers) → calls `onHardwareReturn`, doesn't
///   insert a newline. That's the Mac Catalyst / iPad-keyboard "send".
/// - Hardware Shift+Return → default UITextView behaviour: newline.
/// - Paste → newlines preserved verbatim; no send.
struct ComposerTextField: UIViewRepresentable {
    @Binding var text: String
    let placeholder: String
    @Binding var isFocused: Bool
    let onHardwareReturn: () -> Void
    // PENDINGCREW SHIM (Todo #3 / Todo #9): ⌘V 粘贴拦截 —— true = 已收进附件
    // 托盘、吞掉本次 paste。与 macOS twin 同形。
    var onPasteAttachments: (() -> Bool)? = nil

    /// Cap on auto-grown height — matches the old `lineLimit(1...6)`.
    private static let maxLines: CGFloat = 6

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> ComposerUITextView {
        let tv = ComposerUITextView()
        tv.delegate = context.coordinator
        tv.font = .systemFont(ofSize: Theme.Fonts.scaled(16))
        tv.textColor = UIColor(Theme.Palette.ink)
        tv.tintColor = UIColor(Theme.Palette.accent)
        tv.backgroundColor = .clear
        tv.textContainerInset = UIEdgeInsets(top: 6, left: 0, bottom: 6, right: 0)
        tv.textContainer.lineFragmentPadding = 0
        tv.isScrollEnabled = false   // lets intrinsicContentSize drive height
        tv.returnKeyType = .default  // soft-keyboard Return inserts \n
        tv.onHardwareReturn = {
            // Hop to the next run loop so SwiftUI sees the latest text
            // binding before the parent reads `input` in its send path.
            DispatchQueue.main.async { onHardwareReturn() }
        }
        // PENDINGCREW SHIM (Todo #9): 粘贴拦截挂到 UITextView 子类。
        tv.onPasteAttachments = onPasteAttachments
        // Placeholder is rendered as a child label so its color tracks
        // the surrounding palette (UITextView has no built-in placeholder).
        let ph = UILabel()
        ph.text = placeholder
        ph.font = tv.font
        ph.textColor = UIColor(Theme.Palette.inkMuted).withAlphaComponent(0.6)
        ph.translatesAutoresizingMaskIntoConstraints = false
        tv.addSubview(ph)
        NSLayoutConstraint.activate([
            ph.leadingAnchor.constraint(equalTo: tv.leadingAnchor),
            ph.topAnchor.constraint(equalTo: tv.topAnchor, constant: 6),
        ])
        context.coordinator.placeholder = ph
        context.coordinator.refreshPlaceholder(tv)
        return tv
    }

    func updateUIView(_ uiView: ComposerUITextView, context: Context) {
        context.coordinator.requestedFocus = isFocused
        // PENDINGCREW SHIM (Todo #9): 闭包捕获宿主状态,随宿主重渲染刷新。
        uiView.onPasteAttachments = onPasteAttachments
        // Keep the placeholder copy in sync with a dynamic `placeholder`
        // (crew chat flips it between 发群消息 / 新建 Todo as todoMode toggles).
        if context.coordinator.placeholder?.text != placeholder {
            context.coordinator.placeholder?.text = placeholder
        }
        if uiView.text != text {
            uiView.text = text
            context.coordinator.refreshPlaceholder(uiView)
            context.coordinator.invalidateHeight(uiView)
        }
        // Sync focus state both ways: SwiftUI → UIKit.
        if isFocused, !uiView.isFirstResponder {
            DispatchQueue.main.async { [weak coordinator = context.coordinator, weak uiView] in
                guard coordinator?.requestedFocus == true, let uiView, !uiView.isFirstResponder else { return }
                uiView.becomeFirstResponder()
            }
        } else if !isFocused, uiView.isFirstResponder {
            DispatchQueue.main.async { [weak coordinator = context.coordinator, weak uiView] in
                guard coordinator?.requestedFocus == false, let uiView, uiView.isFirstResponder else { return }
                uiView.resignFirstResponder()
            }
        }
    }

    @MainActor
    func sizeThatFits(_ proposal: ProposedViewSize, uiView: ComposerUITextView, context: Context) -> CGSize? {
        let width = proposal.width ?? UIView.layoutFittingExpandedSize.width
        let fitted = uiView.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude))
        let lineHeight = (uiView.font?.lineHeight ?? 20)
        let maxHeight = lineHeight * Self.maxLines + uiView.textContainerInset.top + uiView.textContainerInset.bottom
        let height = min(fitted.height, maxHeight)
        // Toggle internal scrolling once the text outgrows the cap so
        // the user can still scroll within the bubble.
        if uiView.isScrollEnabled != (fitted.height > maxHeight) {
            uiView.isScrollEnabled = fitted.height > maxHeight
        }
        return CGSize(width: width, height: height)
    }

    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: ComposerTextField
        weak var placeholder: UILabel?
        var requestedFocus = false

        init(_ parent: ComposerTextField) { self.parent = parent }

        func textViewDidChange(_ textView: UITextView) {
            parent.text = textView.text ?? ""
            refreshPlaceholder(textView)
            invalidateHeight(textView)
        }

        func textViewDidBeginEditing(_ textView: UITextView) {
            if !parent.isFocused { parent.isFocused = true }
        }

        func textViewDidEndEditing(_ textView: UITextView) {
            if parent.isFocused { parent.isFocused = false }
        }

        func refreshPlaceholder(_ textView: UITextView) {
            placeholder?.isHidden = !(textView.text?.isEmpty ?? true)
        }

        func invalidateHeight(_ textView: UITextView) {
            textView.invalidateIntrinsicContentSize()
        }
    }
}

/// UITextView subclass that intercepts plain hardware Return so the
/// host can treat it as "send". Shift+Return falls through to the
/// default behaviour (newline). Soft-keyboard Return arrives via the
/// normal text input pipeline (not `pressesBegan`) and is therefore
/// unaffected — it still inserts a newline.
final class ComposerUITextView: UITextView {
    var onHardwareReturn: (() -> Void)?
    // PENDINGCREW SHIM (Todo #9): ⌘V / 长按「粘贴」拦截 —— 剪贴板是图片时宿主
    // 收进附件托盘并返回 true（吞掉本次 paste），纯文本返回 false 走默认粘贴。
    var onPasteAttachments: (() -> Bool)?

    // UITextView 只在剪贴板有**文本**时才让「粘贴」可用（硬件 ⌘V 同理走这条
    // 校验），剪贴板里只有图片时菜单项灰着、paste(_:) 收不到调用 —— 与 macOS
    // 那半（readablePasteboardTypes）是同一个病的两种形态。有图就放行。
    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        if action == #selector(UIResponderStandardEditActions.paste(_:)),
           onPasteAttachments != nil,
           UIPasteboard.general.hasImages {
            return true
        }
        return super.canPerformAction(action, withSender: sender)
    }

    override func paste(_ sender: Any?) {
        if onPasteAttachments?() == true { return }
        super.paste(sender)
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if let key = presses.first?.key,
           key.keyCode == .keyboardReturnOrEnter || key.keyCode == .keypadEnter,
           !key.modifierFlags.contains(.shift),
           !key.modifierFlags.contains(.alternate),
           // markedTextRange != nil means an IME (e.g. 拼音) is mid-
           // composition — Return there commits the candidate, not a
           // send. Fall through so UITextView handles the commit.
           markedTextRange == nil {
            onHardwareReturn?()
            return
        }
        super.pressesBegan(presses, with: event)
    }
}
#elseif os(macOS)

// ── Composer text input (macOS twin) ───────────────────────────────────────

/// NSTextView-backed input mirroring the iOS `ComposerTextField` UITextView
/// twin. A plain SwiftUI `TextField` can't be used here: its bound `String`
/// stays empty while an IME (拼音 etc.) is mid-composition, so a
/// `text.isEmpty` placeholder never hides and ends up painted on top of the
/// candidate letters the user is typing. NSTextView exposes `hasMarkedText()`,
/// which lets the placeholder disappear the moment composition starts.
///
/// Behavior summary (kept identical to the previous SwiftUI version):
/// - Plain hardware Return → calls `onHardwareReturn` (send), no newline.
/// - Shift+Return / Option+Return → inserts a newline.
/// - Grows 1–6 lines, then scrolls internally.
/// - `isFocused` ⇄ first-responder kept in sync both directions.
/// - Placeholder visible iff the field is empty AND not composing (marked text).
struct ComposerTextField: NSViewRepresentable {
    @Binding var text: String
    let placeholder: String
    @Binding var isFocused: Bool
    let onHardwareReturn: () -> Void
    // PENDINGCREW SHIM (Todo #3): ⌘V 粘贴拦截 —— true = 已收进附件托盘、吞掉。
    var onPasteAttachments: (() -> Bool)? = nil

    /// Cap on auto-grown height — matches the old `lineLimit(1...6)`.
    private static let maxLines: CGFloat = 6

    private var font: NSFont { .systemFont(ofSize: Theme.Fonts.scaled(16)) }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSScrollView {
        let scrollView = NSScrollView()
        scrollView.drawsBackground = false
        scrollView.hasVerticalScroller = false
        scrollView.hasHorizontalScroller = false
        scrollView.borderType = .noBorder
        scrollView.verticalScrollElasticity = .allowed

        let tv = ComposerNSTextView()
        tv.delegate = context.coordinator
        tv.font = font
        tv.textColor = NSColor(Theme.Palette.ink)
        tv.insertionPointColor = NSColor(Theme.Palette.accent)
        tv.drawsBackground = false
        tv.isRichText = false
        tv.isEditable = true
        tv.isSelectable = true
        tv.allowsUndo = true
        tv.textContainerInset = NSSize(width: 0, height: 6)
        tv.textContainer?.lineFragmentPadding = 0
        tv.textContainer?.widthTracksTextView = true
        tv.isVerticallyResizable = true
        tv.isHorizontallyResizable = false
        tv.autoresizingMask = [.width]
        tv.minSize = NSSize(width: 0, height: 0)
        tv.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        // Marked-text (IME candidate) changes don't fire the text-did-change
        // delegate, so hook the composition entry points directly to keep the
        // placeholder visibility honest while the user is still typing pinyin.
        tv.onMarkedTextChange = { [weak coordinator = context.coordinator, weak tv] in
            guard let coordinator, let tv else { return }
            coordinator.refreshPlaceholder(tv)
        }
        // Focus tracking hangs off the responder chain, NOT the
        // textDidBegin/EndEditing delegate pair: AppKit only posts
        // textDidBeginEditing on the first actual text change (unlike iOS,
        // where focus alone begins editing). With the delegate pair, a click
        // left `isFocused` false, and the next host re-render (the crew UI
        // ticks ~every second) hit updateNSView's "binding says unfocused but
        // we're first responder" branch and stole focus back — the "click,
        // then focus drops a second later unless you type" bug.
        tv.onFocusChange = { [weak coordinator = context.coordinator] focused in
            guard let coordinator else { return }
            // Keep the async make/resign guards' snapshot in sync too, so a
            // stale queued resign can't fire against the new reality.
            coordinator.requestedFocus = focused
            if coordinator.parent.isFocused != focused {
                coordinator.parent.isFocused = focused
            }
        }
        // PENDINGCREW SHIM (Todo #3): ⌘V 粘贴拦截挂到 NSTextView 子类。
        tv.onPasteAttachments = onPasteAttachments
        scrollView.documentView = tv

        // Placeholder is drawn by the text view itself (see ComposerNSTextView
        // .draw) — NOT an NSTextField subview of the scroll view. Two reasons,
        // both hard-learned: Auto Layout constraints on a direct subview of
        // NSScrollView are never applied (NSScrollView's layout pass is tile(),
        // which skips the constraint engine → the label stays at its initial
        // frame, top-left + truncated), and an NSTextField overlay swallows
        // mouse-downs over exactly the strip where the placeholder text shows,
        // so click-to-focus dies there. Drawing at textContainerOrigin with the
        // same font is pixel-aligned with real text by construction and leaves
        // hit-testing entirely to the text view.
        tv.placeholderString = placeholder
        return scrollView
    }

    func updateNSView(_ nsView: NSScrollView, context: Context) {
        context.coordinator.parent = self
        context.coordinator.requestedFocus = isFocused
        guard let tv = nsView.documentView as? ComposerNSTextView else { return }
        // Keep the drawn placeholder in sync with a dynamic `placeholder`
        // (crew chat flips it between 发群消息 / 新建 Todo as todoMode toggles).
        if tv.placeholderString != placeholder {
            tv.placeholderString = placeholder
            tv.needsDisplay = true
        }
        // PENDINGCREW SHIM (Todo #3): 粘贴拦截闭包随宿主重渲染刷新（闭包捕获宿主状态）。
        tv.onPasteAttachments = onPasteAttachments
        // Don't stomp an in-flight IME composition; only push external text
        // changes (send-clear, @-mention insertion) when not composing.
        if !tv.hasMarkedText(), tv.string != text {
            tv.string = text
            context.coordinator.refreshPlaceholder(tv)
        }
        let isFirst = (tv.window?.firstResponder === tv)
        if isFocused, !isFirst {
            DispatchQueue.main.async { [weak coordinator = context.coordinator, weak tv] in
                guard coordinator?.requestedFocus == true, let tv,
                      let window = tv.window, window.firstResponder !== tv else { return }
                window.makeFirstResponder(tv)
            }
        } else if !isFocused, isFirst {
            DispatchQueue.main.async { [weak coordinator = context.coordinator, weak tv] in
                guard coordinator?.requestedFocus == false, let tv,
                      let window = tv.window, window.firstResponder === tv else { return }
                window.makeFirstResponder(nil)
            }
        }
    }

    @MainActor
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSScrollView, context: Context) -> CGSize? {
        let width = proposal.width ?? 200
        let inset = (nsView.documentView as? NSTextView)?.textContainerInset ?? NSSize(width: 0, height: 6)
        let verticalInset = inset.height * 2
        let attrs: [NSAttributedString.Key: Any] = [.font: font]
        let oneLine = ("M" as NSString).size(withAttributes: attrs).height
        let maxHeight = oneLine * Self.maxLines + verticalInset
        let measured = (nsView.documentView as? NSTextView)?.string ?? text
        let bounding = (measured.isEmpty ? " " : measured as String).boundingRect(
            with: NSSize(width: max(0, width - inset.width * 2), height: CGFloat.greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: attrs
        )
        let content = ceil(bounding.height) + verticalInset
        let height = min(max(content, oneLine + verticalInset), maxHeight)
        return CGSize(width: width, height: height)
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: ComposerTextField
        var requestedFocus = false

        init(_ parent: ComposerTextField) { self.parent = parent }

        func textDidChange(_ notification: Notification) {
            guard let tv = notification.object as? NSTextView else { return }
            parent.text = tv.string
            refreshPlaceholder(tv)
        }

        // NOTE: no textDidBeginEditing/textDidEndEditing here — focus sync
        // rides ComposerNSTextView's become/resignFirstResponder overrides
        // (see onFocusChange in makeNSView). The editing notifications fire
        // too late on AppKit (first keystroke, not focus) to be a focus signal.

        /// Intercept the plain Return so it sends instead of inserting a
        /// newline. Shift/Option+Return fall through to the default newline.
        /// While an IME is composing (`hasMarkedText`), Return is consumed by
        /// the input method to commit the candidate and never reaches here, but
        /// the guard makes that explicit and fail-safe.
        func textView(_ textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
            if commandSelector == #selector(NSResponder.insertNewline(_:)) {
                let flags = NSApp.currentEvent?.modifierFlags ?? []
                let wantsNewline = flags.contains(.shift) || flags.contains(.option)
                if !wantsNewline, !textView.hasMarkedText() {
                    parent.onHardwareReturn()
                    return true
                }
            }
            return false
        }

        func refreshPlaceholder(_ textView: NSTextView) {
            // Visibility is decided inside ComposerNSTextView.draw (empty &&
            // not composing); this just schedules a repaint on state changes.
            textView.needsDisplay = true
        }
    }
}

/// NSTextView subclass that surfaces IME composition changes. `setMarkedText`
/// / `unmarkText` are the entry/exit points for marked (candidate) text and
/// don't trigger `textDidChange`, so we notify from them to keep the
/// placeholder hidden the instant composition starts and restored when it
/// clears.
final class ComposerNSTextView: NSTextView {
    var onMarkedTextChange: (() -> Void)?
    /// Fired from become/resignFirstResponder — the only reliable focus signal
    /// on AppKit (textDidBeginEditing waits for the first keystroke).
    var onFocusChange: ((Bool) -> Void)?

    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        if ok { onFocusChange?(true) }
        return ok
    }

    override func resignFirstResponder() -> Bool {
        let ok = super.resignFirstResponder()
        if ok { onFocusChange?(false) }
        return ok
    }
    // PENDINGCREW SHIM (Todo #3): ⌘V 粘贴拦截 —— 剪贴板是图片/文件时宿主收进
    // 附件托盘并返回 true（吞掉本次 paste），纯文本返回 false 走默认粘贴。
    var onPasteAttachments: (() -> Bool)?
    /// Placeholder drawn in `draw(_:)` when empty and not composing. Drawn (not
    /// an overlay view) so it can't intercept clicks and is positioned at the
    /// exact text origin — see the comment at the construction site.
    var placeholderString: String = ""

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard !placeholderString.isEmpty, string.isEmpty, !hasMarkedText() else { return }
        let attrs: [NSAttributedString.Key: Any] = [
            .font: font ?? .systemFont(ofSize: Theme.Fonts.scaled(16)),
            .foregroundColor: NSColor(Theme.Palette.inkMuted).withAlphaComponent(0.6),
        ]
        let origin = NSPoint(
            x: textContainerOrigin.x + (textContainer?.lineFragmentPadding ?? 0),
            y: textContainerOrigin.y
        )
        NSAttributedString(string: placeholderString, attributes: attrs).draw(at: origin)
    }

    override func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        super.setMarkedText(string, selectedRange: selectedRange, replacementRange: replacementRange)
        onMarkedTextChange?()
    }

    override func unmarkText() {
        super.unmarkText()
        onMarkedTextChange?()
    }

    // PENDINGCREW SHIM (Todo #9): ⌘V 能不能派发下来,由 AppKit 拿
    // `readablePasteboardTypes` 去校验「编辑▸粘贴」菜单项决定 ——
    // `preferredPasteboardType(from: 剪贴板类型, restrictedToTypesFrom:
    // readablePasteboardTypes)` 为 nil 就把菜单项置灰,⌘V 连派发都不会发生。
    // 纯文本 NSTextView(isRichText = false)的默认清单里**没有任何图片类型**
    // (只有 string/RTF/HTML/URL/filenames/color/font/ruler),所以剪贴板里只有
    // 位图时,下面那个 paste(_:) 拦截器永远等不到调用 —— 这正是 Todo #9
    // 「群聊粘不了图、⌘V 毫无反应」的病根(粘文字/粘文件能用,是因为
    // NSStringPboardType / NSFilenamesPboardType 本来就在清单里)。
    // 把宿主收得下的位图类型补进清单,校验才会放行。
    /// 清单本体在 `CrewPasteAttachments.macPasteboardTypes`（单一真值，被单测钉住）。
    override var readablePasteboardTypes: [NSPasteboard.PasteboardType] {
        guard onPasteAttachments != nil else { return super.readablePasteboardTypes }
        return super.readablePasteboardTypes + CrewPasteAttachments.macPasteboardTypes
    }

    // 上面补进去的位图类型只用于放行派发 —— 宿主没收下时(返回 false)不要让
    // NSTextView 真拿位图去 readSelection,否则纯文本框会插进一个奇怪的空附件。
    // 文件类型不拦:宿主没收下(比如文件读不出来)时,退回旧行为把路径当文本粘,
    // 好过静默什么都不发生。
    override func readSelection(from pboard: NSPasteboard, type: NSPasteboard.PasteboardType) -> Bool {
        if type == .png || type == .tiff { return false }
        return super.readSelection(from: pboard, type: type)
    }

    // PENDINGCREW SHIM (Todo #3): 见 onPasteAttachments。
    override func paste(_ sender: Any?) {
        if onPasteAttachments?() == true { return }
        super.paste(sender)
    }
}
#endif
