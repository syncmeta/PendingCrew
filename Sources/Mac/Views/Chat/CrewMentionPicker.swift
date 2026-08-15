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
struct CrewMentionPicker: View {
    let candidates: [CrewMentionCandidate]
    let onPick: (CrewMentionCandidate) -> Void

    var body: some View {
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
        .frame(maxWidth: 260, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Theme.Palette.surface)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Theme.Palette.hairline, lineWidth: 0.5)
        )
        .shadow(color: .black.opacity(0.12), radius: 10, x: 0, y: 4)
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
