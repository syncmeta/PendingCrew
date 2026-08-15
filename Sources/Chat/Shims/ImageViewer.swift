// PENDINGCREW SHIM: ImageViewer
//
// PendingBot's `ImageViewer` lives in Components/ServerImage.swift and uses
// PendingBot's `ServerImage` (Supabase-JWT-gated). In PendingCrew, `ServerImage`
// is already shimmed to CrewRemoteImage (AttachmentDownload + AppModel.imageAuth).
//
// This shim provides an `ImageViewer` with the EXACT init signature BubbleView
// calls:
//
//   ImageViewer(path: target.path, serverURL: serverURL)
//
// It delegates image loading to the vendored `ServerImage` shim (which routes
// through CrewRemoteImage on macOS and shows a placeholder on iOS).
// Zoom/pan gesture and dismiss button are verbatim from PendingBot's original.

import SwiftUI

struct ImageViewer: View {
    let path: String
    let serverURL: URL

    @Environment(\.dismiss) private var dismiss
    @State private var scale: CGFloat = 1
    @State private var steadyScale: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var steadyOffset: CGSize = .zero

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
                .onTapGesture { dismiss() }
            ServerImage(path: path, serverURL: serverURL, contentMode: .fit)
                .scaleEffect(scale)
                .offset(offset)
                .gesture(
                    MagnificationGesture()
                        .onChanged { v in scale = min(max(steadyScale * v, 1), 5) }
                        .onEnded { _ in
                            steadyScale = scale
                            if scale <= 1 { resetPan() }
                        }
                )
                .simultaneousGesture(
                    DragGesture()
                        .onChanged { v in
                            guard scale > 1 else { return }
                            offset = CGSize(
                                width: steadyOffset.width + v.translation.width,
                                height: steadyOffset.height + v.translation.height)
                        }
                        .onEnded { _ in steadyOffset = offset }
                )
                .onTapGesture(count: 2) {
                    withAnimation(.easeInOut(duration: 0.2)) {
                        if scale > 1 {
                            scale = 1; steadyScale = 1; resetPan()
                        } else {
                            scale = 2; steadyScale = 2
                        }
                    }
                }
        }
        .overlay(alignment: .topTrailing) {
            Button { dismiss() } label: {
                Image(systemName: "xmark")
                    .font(Theme.Fonts.glyph(size: 16, weight: .semibold))
                    .foregroundStyle(.white)
                    .padding(10)
                    .background(.black.opacity(0.4), in: Circle())
            }
            // 同 ComposerView 的删除叉：不给 .plain,macOS 会在这枚自绘圆底之外
            // 再套一层系统 bordered 底 + 投影,看起来像一大片灰影。
            .buttonStyle(.plain)
            .padding(.top, 12)
            .padding(.trailing, 16)
        }
        #if os(macOS)
        // macOS 通过大 sheet 呈现(无 fullScreenCover):给个像样的初始尺寸,
        // 并支持 Esc 关闭(对齐"点背景/关闭按钮"两条退出路径)。
        .frame(minWidth: 680, minHeight: 560)
        .onExitCommand { dismiss() }
        #endif
    }

    private func resetPan() {
        offset = .zero
        steadyOffset = .zero
    }
}
