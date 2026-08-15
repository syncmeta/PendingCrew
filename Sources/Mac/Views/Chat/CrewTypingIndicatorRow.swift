#if os(macOS)
import SwiftUI

/// iMessage 式「正在输入」指示（Todo #4，本地模式）。
///
/// 每个本 crew 的活跃 run 一行：session 头像/名字 + 三点动画气泡，渲染在
/// `CrewChatView` 消息列表底部。**行自己观察 run**（`@ObservedObject`）——
/// `isWorking` 是 `CrewSessionRun` 上的 `@Published`，父视图只观察 runner
/// （runs 数组增删），不转发子对象变更；把「显示/消失」的判定放进行内，
/// runner 状态一翻（跑回合 ↔ 空闲）本行立即自增自灭，无需父视图刷新。
///
/// 只做本地模式：进程内 runner 状态事件驱动，不做 edge 广播版（拍板 #4）。
struct CrewTypingIndicatorRow: View {
    @ObservedObject var run: CrewSessionRun
    /// captain 头像种子对齐 roster 的 captainBotId（同 `senderForRun`），
    /// 否则机长「在跑/没跑」两张脸。
    var captainBotId: String? = nil
    /// 出现时回调（父视图滚到底部，让指示器可见）。
    var onShow: () -> Void = {}

    /// 干活中才显示 —— `.running` 且回合进行中；离开该状态即消失。
    /// 用 `displayIsTyping` 而不是 `isWorking`（Todo #24）：后者是「最近 1s 有 PTY
    /// 输出」的原始信号，claude 空闲时的界面心跳重绘每次都点亮它 → 气泡亮一下灭
    /// 一下循环往复。`displayIsTyping` 只认「画面可见文本真的变了」，并带熄灭迟滞。
    private var isTyping: Bool { run.status == .running && run.displayIsTyping }

    var body: some View {
        Group {
            if isTyping {
                row
            }
        }
        .onAppear { if isTyping { onShow() } }
        .onChange(of: isTyping) { _, now in
            if now { onShow() }
        }
    }

    /// 布局对齐 BubbleView 的 bot 行（头像 30 + 名字 + 气泡），视觉上是
    /// 「同一个人正在打一条消息」。
    private var row: some View {
        HStack(alignment: .top, spacing: 8) {
            CrewAvatarBadges(sender: sender, size: 30)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 4) {
                Text(sender.displayName)
                    .font(Theme.Fonts.rounded(size: 11, weight: .semibold))
                    .foregroundStyle(Theme.Palette.inkMuted)
                CrewTypingDots()
                    .padding(.horizontal, 14)
                    .padding(.vertical, 12)
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
            }
            Spacer(minLength: 32)
        }
        .padding(.horizontal, Theme.Metrics.gutter)
        .padding(.vertical, 4)
        .transition(.opacity)
    }

    private var sender: GroupBubbleSender {
        let isCaptain = run.role == .captain
        let seed = isCaptain ? (captainBotId ?? run.sessionId) : run.sessionId
        var s = GroupBubbleSender(
            kind: .bot, id: seed, displayName: run.displayName,
            avatarPath: nil, avatarSeed: seed)
        s.isCaptain = isCaptain
        s.isSession = true
        return s
    }
}

/// 三点跳动动画：三个圆点相位错开的呼吸透明度（绘制与动画都在 CoreAnimation 里）。
struct CrewTypingDots: View {
    /// 呼吸动画交给 CoreAnimation（见 `TypingDotsLayerView` 顶部那段病根说明）——
    /// SwiftUI 侧的 `.repeatForever` 会和群聊 ScrollView 的滚动锚点/程序化 scrollTo
    /// 组成自激环，把主窗口的约束计数顶爆导致闪退（2026-07-26 17:24）。
    var body: some View {
        TypingDotsLayerView(color: NSColor(Theme.Palette.inkMuted))
            .frame(width: TypingDotsLayerView.intrinsicSize.width,
                   height: TypingDotsLayerView.intrinsicSize.height)
    }
}
#endif
