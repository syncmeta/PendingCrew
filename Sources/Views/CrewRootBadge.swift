import SwiftUI

/// crew 名字后面那行**黄字**根 crew 标注 —— `Crew出iOS @PendingCrew`（企业微信
/// 联系人后面挂部门那种感觉）。三端共用这一个视图，别各写各的。
///
/// 规矩（2026-08-11 机长拍板，别再改口径）：
/// - 标的是**根祖先**，不是直接父。`C ← B ← A` 时 C 标 `@A`。
/// - 能放下时多父全显示；放不下按加入顺序保留完整前 N 个，再写准确的 `+N`。
/// - 黄字绝不半截：连第一个完整父名都放不下就整条不画。
/// - 名字至少保留 4–6 个显示字符的实测宽度；发生截断一定用尾省略号。
/// - 名字下限与黄字冲突时，名字优先、整条黄字撤掉。
/// - crew 自己就是根 → 不画（`badgeText` 返回 nil，本视图渲染成空）。
struct CrewRootBadge: View {
    /// 根 crew 标题（已按加入顺序排好）—— 由 `CrewRootLineage.rootTitles` 给。
    let rootTitles: [String]
    /// 字号跟随宿主行：侧栏 12、iPad/iOS 列表 12、需要更小的地方自己传。
    var size: CGFloat = 12

    var body: some View {
        let candidates = CrewRootBadgePresentation.candidateTexts(rootTitles: rootTitles)
        if !candidates.isEmpty {
            ViewThatFits(in: .horizontal) {
                ForEach(Array(candidates.enumerated()), id: \.offset) { _, text in
                    Text(text)
                        // ViewThatFits 量的是每条候选的真实完整宽度；Text 自己绝不截断。
                        .fixedSize(horizontal: true, vertical: false)
                        .accessibilityHidden(true)
                }
                // 所有文字候选都放不下时，ViewThatFits 选中这个零尺寸兜底，
                // 等价于整条黄字不画。
                Color.clear
                    .frame(width: 0, height: 0)
                    .accessibilityHidden(true)
            }
                .font(Theme.Fonts.system(size: size))
                .foregroundStyle(Theme.Palette.amber)
                .lineLimit(1)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("挂在 \(rootTitles.joined(separator: "、")) 之下")
                .help("这个 crew 挂在 \(rootTitles.joined(separator: "、")) 之下")
        }
    }
}

/// crew 名字和根归属黄字的联合布局。第三个（隐藏）子视图是与标题同字体的宽度
/// 探针；布局先实测它，再只把余量交给黄字的 `ViewThatFits`。
private struct CrewTitleRootBadgeLayout: Layout {
    let spacing: CGFloat

    private struct Measurement {
        let title: ViewDimensions
        let badge: ViewDimensions
        let titleWidth: CGFloat
        let badgeWidth: CGFloat
        let width: CGFloat
        let height: CGFloat
        let baseline: CGFloat
    }

    func sizeThatFits(
        proposal: ProposedViewSize,
        subviews: Subviews,
        cache: inout ()
    ) -> CGSize {
        guard subviews.count == 3 else { return .zero }
        let measured = measure(proposal: proposal, subviews: subviews)
        return CGSize(width: measured.width, height: measured.height)
    }

    func placeSubviews(
        in bounds: CGRect,
        proposal: ProposedViewSize,
        subviews: Subviews,
        cache: inout ()
    ) {
        guard subviews.count == 3 else { return }
        let measured = measure(
            proposal: ProposedViewSize(width: bounds.width, height: proposal.height),
            subviews: subviews
        )
        let titleY = bounds.minY + measured.baseline - measured.title[.firstTextBaseline]
        subviews[0].place(
            at: CGPoint(x: bounds.minX, y: titleY),
            anchor: .topLeading,
            proposal: ProposedViewSize(width: measured.titleWidth, height: nil)
        )

        let badgeY = bounds.minY + measured.baseline - measured.badge[.firstTextBaseline]
        subviews[1].place(
            at: CGPoint(x: bounds.minX + measured.titleWidth + (measured.badgeWidth > 0 ? spacing : 0), y: badgeY),
            anchor: .topLeading,
            proposal: ProposedViewSize(width: measured.badgeWidth, height: nil)
        )

        // 探针参与测量但永远不画；仍明确放置，避免自定义 Layout 漏放子视图时
        // SwiftUI 沿用默认位置造成重叠。
        subviews[2].place(
            at: CGPoint(x: bounds.minX, y: bounds.minY),
            anchor: .topLeading,
            proposal: .zero
        )
    }

    private func measure(proposal: ProposedViewSize, subviews: Subviews) -> Measurement {
        let titleIdeal = subviews[0].dimensions(in: .unspecified)
        let badgeIdeal = subviews[1].dimensions(in: .unspecified)
        let titleFloor = subviews[2].dimensions(in: .unspecified).width
        let requiredTitle = min(titleIdeal.width, titleFloor)
        let idealWidth = titleIdeal.width + (badgeIdeal.width > 0 ? spacing + badgeIdeal.width : 0)
        // 自己报告的最小宽度就是标题下限；即使父布局给得更窄，也不能重新把名字
        // 压成空或一个字。正常三端行宽都大于这个下限。
        let available = max(proposal.width ?? idealWidth, requiredTitle)
        let badgeBudget = max(0, available - requiredTitle - spacing)
        let badge = subviews[1].dimensions(
            in: ProposedViewSize(width: badgeBudget, height: proposal.height)
        )
        let showsBadge = badge.width > 0.5
        let titleBudget = max(0, available - (showsBadge ? badge.width + spacing : 0))
        let title = subviews[0].dimensions(
            in: ProposedViewSize(width: titleBudget, height: proposal.height)
        )

        let titleBaseline = title[.firstTextBaseline]
        let badgeBaseline = showsBadge ? badge[.firstTextBaseline] : titleBaseline
        let baseline = max(titleBaseline, badgeBaseline)
        let belowBaseline = max(
            title.height - titleBaseline,
            showsBadge ? badge.height - badgeBaseline : 0
        )
        let usedWidth = title.width + (showsBadge ? spacing + badge.width : 0)
        return Measurement(
            title: title,
            badge: badge,
            titleWidth: titleBudget,
            badgeWidth: showsBadge ? badge.width : 0,
            width: min(available, usedWidth),
            height: baseline + belowBaseline,
            baseline: baseline
        )
    }
}

/// 三端行复用的“名字 + 黄字”。标题的省略号由 SwiftUI 在真实分配宽度里生成；
/// 下限由同字体隐藏探针实测，不按中英文字符数切字符串。
struct CrewTitleRootBadge: View {
    let title: String
    let titleFont: Font
    let titleColor: Color
    let rootTitles: [String]
    var badgeSize: CGFloat = 12
    var spacing: CGFloat = 6

    private var titleText: some View {
        Text(title)
            .font(titleFont)
            .foregroundStyle(titleColor)
            .lineLimit(1)
            .truncationMode(.tail)
    }

    @ViewBuilder
    var body: some View {
        let usableRoots = rootTitles.filter {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
        if usableRoots.isEmpty {
            // 根 crew 没有黄字；沿用普通标题布局，别让零宽 badge/探针参与父级对齐。
            titleText
        } else {
            CrewTitleRootBadgeLayout(spacing: spacing) {
                titleText
                CrewRootBadge(rootTitles: usableRoots, size: badgeSize)
                Text(CrewRootBadgePresentation.titleMinimumWidthProbe)
                    .font(titleFont)
                    .lineLimit(1)
                    .fixedSize(horizontal: true, vertical: false)
                    .hidden()
                    .accessibilityHidden(true)
            }
            .accessibilityElement(children: .contain)
        }
    }
}
