import SwiftUI

/// `BotAvatar`(PendingBot 同款 emoji+柔色头像)+ crew 角标(captain 星标 /
/// session 运行状态点),driven by `GroupBubbleSender`。
///
/// Replaces the old `CrewAvatar` at the bubble avatar slot (A8 wires it in;
/// A11 deletes `CrewAvatar`). Cross-platform — no `#if os(macOS)` gate.
///
/// 头像本体:bot / captain / code session 一律走 `BotAvatar(emojiSeed:colorSeed:)`
/// —— 和 PendingBot 里 bot 头像同一套生成法,同一 id 永远同一张脸。session 不再用
/// 专门的 terminal 图标,改由运行状态点(bot 没有)区分。
///
/// Badge geometry（人类定调 2026-08-08：两个点合成一个）：
/// - Captain star:  `star.fill`, orange, size×0.30 bold, 无底(裸星),
///   offset (-size×0.34, -size×0.34) → top-leading quadrant。保留。
/// - Status dot:    plain circle size×0.26, canvas stroke 1.5 pt,
///   offset (+size×0.34, +size×0.34) → bottom-trailing quadrant。
///   语义与配色收口在 `SessionStatusDot`（绿=干活 / 黄=空闲 / 红=需要人出手 /
///   灰=已退出）；红态呼吸（CoreAnimation，见 `BreathingDot`）。nil 时不画。
///
/// 右上角原本还有一颗「注意红点」（`hasNotification`：未读 ∨ 异常 ∨ 进程死），
/// 已删 —— 它三种点亮情形里有两种右下角本来就已经变红，重复且互相打架；未读
/// 也不该再点亮任何点（只留切换条上的数字角标）。
struct CrewAvatarBadges: View {
    private static let pendingCrewAppImageReduction: CGFloat = 4

    let sender: GroupBubbleSender
    var size: CGFloat = 30

    /// PendingCrew 的系统消息保留与普通头像相同的槽位，只缩小内部 App 图标。
    private var baseImageSize: CGFloat {
        sender.isPendingCrewApp ? size - Self.pendingCrewAppImageReduction : size
    }

    var body: some View {
        ZStack {
            base
            if sender.isCaptain { captainStar }
            statusDot
        }
        .frame(width: size, height: size)
    }

    // MARK: - Base avatar

    @ViewBuilder private var base: some View {
        // 所有成员 —— bot / captain / code session —— 一律走 PendingBot 同款
        // BotAvatar:emoji 种子 = participant id(稳定身份)、色种子 = avatarSeed
        // (按群着色)。session 不再用专门的 terminal 图标,改由下面的运行状态点
        // (bot 没有)区分。
        if sender.isPendingCrewApp {
            // AppIcon.appiconset 不能作为普通命名图片读取；BrandMark 是同一套 App
            // 图形的可渲染 imageset，白底圆形裁切后用于群聊头像。
            Image("BrandMark")
                .resizable()
                .scaledToFill()
                .frame(width: baseImageSize, height: baseImageSize)
                .background(Color.white, in: Circle())
                .clipShape(Circle())
                .overlay(Circle().strokeBorder(Theme.Palette.hairline, lineWidth: 0.5))
        } else {
            BotAvatar(emojiSeed: sender.id, colorSeed: sender.avatarSeed, size: size)
        }
    }

    // MARK: - Captain star badge (top-leading)
    //
    // 用户定调(Todo #9):裸星、无底色,挪到左上角 —— 右上角让给注意红点。

    private var captainStar: some View {
        Image(systemName: "star.fill")
            .font(.system(size: size * 0.30, weight: .bold))
            .foregroundStyle(.orange)
            .offset(x: -(size * 0.34), y: -(size * 0.34))
    }

    // MARK: - Session status dot (bottom-trailing) —— 头像上唯一的状态点
    //
    // 语义/配色不在这儿判，一律走 `SessionStatusDotDerivation`（那里是唯一推导，
    // 有单测）。这里只负责画：圆点 + canvas 描边圈，红态换成呼吸版。

    @ViewBuilder private var statusDot: some View {
        if let dot = SessionStatusDotDerivation.dot(state: sender.sessionStatus) {
            Group {
                if dot.breathes {
                    // 呼吸走 CoreAnimation（`BreathingDot`），绝不用 SwiftUI 的
                    // repeatForever —— 那条路会把布局顶成自激死循环（见该文件验尸）。
                    BreathingDot(size: size * 0.26, color: dot.color)
                } else {
                    Circle().fill(dot.color).frame(width: size * 0.26, height: size * 0.26)
                }
            }
            // 描边圈是静态 SwiftUI 图层，不参与动画 —— 呼吸只发生在图层的 opacity 上。
            .overlay(Circle().strokeBorder(Theme.Palette.canvas, lineWidth: 1.5))
            .frame(width: size * 0.26, height: size * 0.26)
            .offset(x: size * 0.34, y: size * 0.34)
        }
    }
}
