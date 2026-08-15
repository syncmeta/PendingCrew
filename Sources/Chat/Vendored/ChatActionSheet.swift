// VENDORED from PendingBot apps/pendingbot/Sources/Features/Message/ChatActionSheet.swift @ 43c8ea2e
// 再对齐 = 对照源文件重拷。仅允许的偏离打 `// PENDINGCREW SHIM:` 标注。
import SwiftUI
#if os(iOS)
import UIKit
#endif

/// WeChat-style "+" action panel for the chat composer. Renders inline
/// below the input row (iMessage / WeChat style), so tapping the
/// composer's "+" pushes the input up and reveals this panel where the
/// keyboard would have been. `onDismiss` lets the host close the panel
/// (e.g. after a photo pick) without the panel needing its own modal
/// context.
///
/// Slimmed down — model picker + skills moved out to BotConfigView (the
/// gear icon in the chat header now lands there directly for bot chats;
/// human chats go to ContactSettingsView, group chats to GroupSettingsView).
/// The remaining tiles are the per-message attach actions: 图片 (library)
/// and 拍照 (camera).
struct ChatActionSheet: View {
    /// Tapping the "图片" tile calls this; the host presents `.photosPicker`.
    var onPickPhoto: () -> Void = {}
    /// Tapping the "拍照" tile calls this; the host presents the camera cover.
    var onPickCamera: () -> Void = {}
    /// Tapping the "文件" tile calls this; the host presents the system
    /// document picker (any file type).
    var onPickFile: () -> Void = {}
    var onDismiss: () -> Void = {}
    /// When non-nil, renders a "检查" tile that manually fires a lookback
    /// on the current conv. Nil for conv types without a single bot (group,
    /// user_user) — those skip the tile entirely.
    var onLookback: (() -> Void)? = nil

    // All pickers (photo / camera / file) are presented by the host from the
    // main view tree via bindings, not from here — this panel lives inside the
    // composer's inputAccessoryView, and presenting a sheet / fullScreenCover
    // from a hosting controller embedded in the accessory is unreliable.

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .top, spacing: 18) {
                // PENDINGCREW SHIM: 「图片」tile 只在 iOS 渲染。宿主的
                // `.photosPicker` 被 `#if os(iOS)` 围着，所以在 Mac 上这颗 tile
                // 点了**毫无反应** —— 它就是"群聊发不了文件"这个体感的直接来源。
                // Mac 没有照片图库这个概念，「文件」入口本来就能选图片，故直接不渲染。
                #if os(iOS)
                Button {
                    Haptics.tap()
                    onPickPhoto()
                    onDismiss()
                } label: {
                    actionTileLabel(icon: "photo.on.rectangle.angled", label: "图片")
                }
                .buttonStyle(.plain)
                #endif

                #if os(iOS)
                // Camera capture is iOS-only — Macs have no camera path
                // (UIImagePickerController is unavailable on macOS).
                Button {
                    Haptics.tap()
                    onPickCamera()
                    onDismiss()
                } label: {
                    actionTileLabel(icon: "camera", label: "拍照")
                }
                .buttonStyle(.plain)
                #endif

                Button {
                    Haptics.tap()
                    onPickFile()
                    onDismiss()
                } label: {
                    actionTileLabel(icon: "doc", label: "文件")
                }
                .buttonStyle(.plain)

                if let onLookback {
                    Button {
                        Haptics.tap()
                        onLookback()
                        onDismiss()
                    } label: {
                        actionTileLabel(icon: "checkmark.seal", label: "检查")
                    }
                    .buttonStyle(.plain)
                }

                Spacer(minLength: 0)
            }
            .padding(.horizontal, Theme.Metrics.gutter)
            .padding(.top, 18)
            .padding(.bottom, 18)

            Spacer(minLength: 0)
        }
        .background(Theme.Palette.canvas)
    }

    private func actionTileLabel(icon: String, label: String) -> some View {
        VStack(spacing: 6) {
            Image(systemName: icon)
                .font(Theme.Fonts.glyph(size: 22, weight: .regular))
                .foregroundStyle(Theme.Palette.accent)
                .frame(width: 56, height: 56)
                .background(
                    RoundedRectangle(cornerRadius: 14, style: .continuous)
                        .fill(Theme.Palette.surface)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 14, style: .continuous)
                        .strokeBorder(Theme.Palette.hairline, lineWidth: 0.5)
                )
            Text(label)
                .font(Theme.Fonts.rounded(size: 11, weight: .medium))
                .foregroundStyle(Theme.Palette.inkMuted)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .frame(width: 64)
    }
}

#if os(iOS)
/// Thin UIImagePickerController wrapper for the camera. PhotosPicker is
/// already used for library selection; UIKit's picker is the cleanest
/// way to get a one-shot camera capture without dragging in AVFoundation.
/// iOS-only — `UIImagePickerController` doesn't exist on macOS and Macs
/// have no camera capture path here.
struct CameraPicker: UIViewControllerRepresentable {
    var onPick: (UIImage?) -> Void

    func makeUIViewController(context: Context) -> UIImagePickerController {
        let picker = UIImagePickerController()
        picker.sourceType = UIImagePickerController.isSourceTypeAvailable(.camera) ? .camera : .photoLibrary
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ uiViewController: UIImagePickerController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(onPick: onPick) }

    final class Coordinator: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
        let onPick: (UIImage?) -> Void
        init(onPick: @escaping (UIImage?) -> Void) { self.onPick = onPick }

        func imagePickerController(_ picker: UIImagePickerController,
                                   didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]) {
            let image = (info[.editedImage] as? UIImage) ?? (info[.originalImage] as? UIImage)
            onPick(image)
        }

        func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
            onPick(nil)
        }
    }
}
#endif
