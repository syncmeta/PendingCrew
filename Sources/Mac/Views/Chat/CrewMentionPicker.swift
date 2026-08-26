import SwiftUI

/// 群聊 composer 的 @-mention autocomplete popover(Phase 6 单元 1)。
///
/// 纯展示 + 选择:候选由 `CrewChatView` 用 `crewMentionCandidates(...)` 从 roster
/// 算好传入(已按当前 @prefix 过滤);选中一项 → 回调 `onPick`,由宿主在 draft 里
/// 插入 token + stage mention。不持有任何状态、不碰 vendored `ComposerView` ——
/// 宿主把它 overlay 在 composer 上方,靠 `$draft` 的 `.onChange` 驱动显隐。
///
/// 形态对齐本仓 Theme:surface 底 + hairline 描边 + 轻阴影的浮层,每行一个
/// kind 图标 + 名字。键盘导航(↑↓/回车)在 v1 不做 —— 鼠标/触摸点选即可,
/// 已记 tech-debt 候选(非阻塞)。
///
/// **限高 + 内部滚动**(Todo #69):候选条数 = 成员数,本机 crew 动辄四十几个人,
/// 原来这里一条上限都没有,列表直接顶穿窗口。高度不写死 —— 由 `availableHeight`
/// (宿主量出的 composer 上方剩余空间)喂给纯函数 `CrewMentionPickerLayout` 算,
/// 政策与三道边全在那边、有单测钉着。这里只负责「把算出来的数扣上去」。
struct CrewMentionPicker: View {
    let candidates: [CrewMentionCandidate]
    /// 宿主量出的「composer 上方可用高度」。`<= 0` = 还没量到,走兜底上限
    /// (仍然有界 —— 量不到也不许回到顶穿)。
    var availableHeight: CGFloat = 0
    let onPick: (CrewMentionCandidate) -> Void

    /// 这次的高度上限。恒 ≤ 内容高度,所以候选少时与改动前逐字相同。
    private var maxHeight: CGFloat {
        CrewMentionPickerLayout.maxHeight(
            availableHeight: availableHeight, rowCount: candidates.count)
    }

    var body: some View {
        // ScrollView 只在超上限时才真的能滚(`.basedOnSize` 让没超时不橡皮筋),
        // 超了则由 `CrewMentionPickerLayout` 保证底下露半行 —— macOS 滚动条默认
        // 隐藏,不露那半行人根本看不出还有更多。
        ScrollView(.vertical) {
            rows
        }
        .scrollBounceBehavior(.basedOnSize)
        // 宽度与改动前逐字相同（行里的 `Spacer(minLength: 0)` 本来就让每行占满
        // 260）；高度这一维才是本条改的东西。
        .frame(maxWidth: 260, maxHeight: maxHeight, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Theme.Palette.surface)
        )
        // 裁在圆角上 —— 滚动内容不许从圆角外面漏出来。
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Theme.Palette.hairline, lineWidth: 0.5)
        )
        .shadow(color: .black.opacity(0.12), radius: 10, x: 0, y: 4)
    }

    /// 候选行本体（ScrollView 的内容）。
    @ViewBuilder
    private var rows: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(candidates) { cand in
                Button {
                    onPick(cand)
                } label: {
                    HStack(spacing: 8) {
                        // 成员候选用真头像（与成员列表/气泡同 seed 同脸）；
                        // broadcast 没有头像概念,保留 megaphone glyph。
                        if let seed = cand.avatarSeed {
                            CrewAvatarBadges(sender: GroupBubbleSender(
                                kind: cand.kind == .human ? .user : .bot,
                                id: seed, displayName: cand.label,
                                avatarPath: nil, avatarSeed: seed,
                                isCaptain: cand.kind == .captain,
                                sessionStatus: nil,
                                isSession: cand.kind == .session), size: 20)
                        } else {
                            Image(systemName: glyph(cand.kind))
                                .font(Theme.Fonts.glyph(size: 13))
                                .foregroundStyle(tint(cand.kind))
                                .frame(width: 20)
                        }
                        Text(cand.label)
                            .font(Theme.Fonts.footnote)
                            .foregroundStyle(Theme.Palette.ink)
                            .lineLimit(1)
                        Spacer(minLength: 0)
                        Text(subtitle(cand.kind))
                            .font(Theme.Fonts.caption2)
                            .foregroundStyle(Theme.Palette.inkMuted)
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 7)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                if cand.id != candidates.last?.id {
                    Divider().opacity(0.4)
                }
            }
        }
        .padding(.vertical, 2)
    }

    private func glyph(_ k: CrewMentionCandidate.Kind) -> String {
        switch k {
        case .broadcast: return "megaphone"
        case .captain:   return "star.circle"
        case .session:   return "terminal"
        case .human:     return "person"
        }
    }

    private func tint(_ k: CrewMentionCandidate.Kind) -> Color {
        switch k {
        case .broadcast, .captain: return Theme.Palette.accent
        default:                   return Theme.Palette.inkMuted
        }
    }

    private func subtitle(_ k: CrewMentionCandidate.Kind) -> String {
        switch k {
        case .broadcast: return "全员"
        case .captain:   return "机长"
        case .session:   return "会话"
        case .human:     return "成员"
        }
    }
}
