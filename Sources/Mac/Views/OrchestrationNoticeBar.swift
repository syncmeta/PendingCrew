#if os(macOS)
import SwiftUI

/// **「这个窗口现在到底管不管事」的那一条横幅**（前后端分离 §6.2 闸门 2）。
///
/// ## 它不是装饰，它是这道闸门的最后一段
///
/// `OrchestrationGate` 让第二个编排者当场被拒 —— 但被拒之后如果没人说出来，
/// 用户看到的就是**一个界面在、什么都不动、不报错的窗口**。这一整期修的正是这种
/// 静默（2026-08-26 那次是「daemon 悄悄跑在真目录上」），而闸门自己的失败形态是
/// **方向反过来的同一种**。所以拒绝必须有出口。
///
/// 落这条之前，闸门的裁决和 `ViewerSessionClient.isConnected` / `.lastError`
/// **全仓零消费者** —— 三个 `@Published` 谁都没读。「有 `@Published` 就等于说出来
/// 了」是假的：没有视图读的 `@Published` 和一句没写的日志是同一个东西。
///
/// ## 这个文件里没有判断
///
/// 显示哪一态由 `OrchestrationNotice.resolve(decision:viewer:)` 算 —— 一个纯函数，
/// 有测试盯着。**GUI 我们验不了**（不许为验证开窗口），所以判断一旦长在视图里就
/// 又变成没人验的东西。这里只负责把已经算好的态画出来。
struct OrchestrationNoticeBar: View {
    @EnvironmentObject private var sessionHost: SessionHost

    var body: some View {
        // 有 viewer 那条腿时必须 `@ObservedObject` 它，否则连上/断开不会重画。
        if let viewer = sessionHost.viewer {
            LinkedNoticeBar(decision: sessionHost.orchestrationDecision, client: viewer)
        } else {
            NoticeBar.render(
                OrchestrationNotice.resolve(
                    decision: sessionHost.orchestrationDecision, viewer: nil))
        }
    }
}

private struct LinkedNoticeBar: View {
    let decision: OrchestrationGate.Decision?
    @ObservedObject var client: ViewerSessionClient

    var body: some View {
        NoticeBar.render(
            OrchestrationNotice.resolve(
                decision: decision,
                viewer: .init(isConnected: client.isConnected, lastError: client.lastError,
                              fallback: client.fallback)))
    }
}

private struct NoticeBar: View {
    let symbol: String
    let tint: Color
    let background: Color
    let title: String
    let detail: String

    /// 态 → 画面。**这里不做判断**，`.none` 时渲染成空，不占一个像素。
    @ViewBuilder
    static func render(_ notice: OrchestrationNotice) -> some View {
        switch notice {
        case .none:
            EmptyView()
        case let .conflict(detail):
            NoticeBar(symbol: "exclamationmark.triangle.fill",
                      tint: Theme.Palette.danger,
                      background: Theme.Palette.dangerBg,
                      title: "本窗口没有接管后台编排",
                      detail: detail)
        case let .connecting(detail):
            NoticeBar(symbol: "bolt.horizontal.circle.fill",
                      tint: Theme.Palette.amber,
                      background: Theme.Palette.amberBg,
                      title: "正在连接后台进程…",
                      detail: detail)
        case let .localFallback(detail):
            // **一直挂着**，不是弹一下就没 —— 临时模式必须随时看得出来。
            NoticeBar(symbol: "arrow.triangle.2.circlepath.circle.fill",
                      tint: Theme.Palette.amber,
                      background: Theme.Palette.amberBg,
                      title: "后台起不来，已临时由本窗口接管",
                      detail: detail)
        case let .refused(detail):
            NoticeBar(symbol: "exclamationmark.triangle.fill",
                      tint: Theme.Palette.danger,
                      background: Theme.Palette.dangerBg,
                      title: "后台连不上，本窗口不接管编排",
                      detail: detail)
        }
    }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: symbol)
                .font(Theme.Fonts.glyph(size: 13))
                .foregroundStyle(tint)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(tint)
                if !detail.isEmpty {
                    Text(detail)
                        .font(Theme.Fonts.caption)
                        .foregroundStyle(Theme.Palette.inkMuted)
                        // 不 `lineLimit` —— 「谁占着」那三样缺一样人就得再查一轮，
                        // 截断等于缺。可选中是为了让 pid / 路径能直接复制去查。
                        .textSelection(.enabled)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, Theme.Metrics.gutter)
        .padding(.vertical, 7)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(background)
        .overlay(alignment: .bottom) {
            Rectangle().fill(Theme.Palette.hairline).frame(height: 1)
        }
    }
}
#endif
