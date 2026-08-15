#if os(macOS)
import SwiftUI

/// 侧栏底部的订阅额度显示（人类 Todo #8 + #10，设计定稿见机长效果图
/// `claude.ai/code/artifact/0434453d-…`）。
///
/// 形态：**每家一行** —— 品牌 logomark 在前，后面一排进度环。
/// - Claude 三个环：5h（滚动窗）/ 周（全模型）/ 当前模型（长条，环里写模型名）
/// - Codex 一个环：周（codex 侧已无 5 小时分档）
/// - 某个窗拿不到数据就**不画那个环**（不画空环占位——空环看着像坏了）
///
/// 环里平时是窗名，鼠标停上去窗名淡出、百分比淡入（各 100ms；reduce-motion
/// 下直接切换不做淡入淡出）。**重置时刻不常驻**，留在整块的 `.help()` 悬停提示
/// 里 —— 「几点回血」跟「用了多少」是两回事，不该抢环里那一格。
///
/// 「画哪些环 / 环里写什么 / 哪一档颜色 / 快照多旧」的判定全在
/// `QuotaRingLayout`（纯 Foundation，单测钉死），这里只负责画。
struct QuotaRingsFooter: View {
    @ObservedObject var quota: QuotaCenter

    /// 刷新按钮转一圈用的角度累加值（每点一次 +360）。
    @State private var refreshSpin: Double = 0
    /// 鼠标当前停在哪个环上（Todo #14）。状态提到这一层而不是留在单个环里：
    /// 环下面那行要跟着换成**那一项**的重置时刻，得知道是哪一个。
    @State private var hoveredRingID: String?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var claudeRings: [QuotaRing] { QuotaRingLayout.claudeRings(quota.claude) }
    private var codexRings: [QuotaRing] { QuotaRingLayout.codexRings(quota.codex) }

    /// 「这一家有话要说」——有环要画，或者虽然没数但得说一句「读不到」。
    private var claudeWarning: String? {
        QuotaRingLayout.warningBadge(quota.claude, failure: quota.claudeError)
    }
    private var codexWarning: String? {
        QuotaRingLayout.warningBadge(quota.codex, failure: quota.codexError)
    }

    var body: some View {
        if !claudeRings.isEmpty || !codexRings.isEmpty
            || quota.claudeError != nil || quota.codexError != nil {
            VStack(alignment: .leading, spacing: 2) {
                if !claudeRings.isEmpty || quota.claudeError != nil {
                    agentRow(asset: "ClaudeLogomark", tint: Theme.Palette.claudeMark,
                             brand: "Claude Code", rings: claudeRings,
                             staleBadge: claudeWarning)
                }
                if !codexRings.isEmpty || quota.codexError != nil {
                    agentRow(asset: "OpenAILogomark", tint: Theme.Palette.openAIMark,
                             brand: "Codex", rings: codexRings,
                             staleBadge: codexWarning)
                }
                freshnessRow
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .help(QuotaRingLayout.helpText(claude: quota.claude, codex: quota.codex,
                                           claudeError: quota.claudeError,
                                           codexError: quota.codexError) ?? "")
        }
    }

    /// 一家一行：logomark + 一排环（+ 读不到 / 窗口已翻篇 / 数据太旧时的警示标记）。
    ///
    /// 警示按**家**挂，不能靠下面那行「N 分钟前」代劳：那行取的是两家里最新的
    /// 读取时刻，claude 刚查过就会把 codex 一个多月没动的数字一起盖成「刚刚」。
    /// 环一个都没有、只剩一句「读不到」时这行也照画 —— 整行消失等于把失败藏起来。
    private func agentRow(asset: String, tint: Color, brand: String,
                          rings: [QuotaRing], staleBadge: String?) -> some View {
        HStack(spacing: 9) {
            Image(asset)
                .renderingMode(.template)
                .resizable()
                .scaledToFit()
                .frame(width: 15, height: 15)
                .foregroundStyle(tint)
                .accessibilityLabel(brand)
            HStack(spacing: 8) {
                ForEach(rings) { ring in
                    QuotaRingView(ring: ring, hoveredRingID: $hoveredRingID)
                }
            }
            if let staleBadge {
                Text("⚠︎ \(staleBadge)")
                    .font(.system(size: 9.5))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .accessibilityLabel("\(brand) 额度\(staleBadge)，不是当前值")
            }
        }
        .padding(.vertical, 1)
    }

    /// 鼠标当前停着的那个环（两家的环合起来找）。
    private var hoveredRing: QuotaRing? {
        guard let id = hoveredRingID else { return nil }
        return (claudeRings + codexRings).first { $0.id == id }
    }

    /// 环下面左对齐的一行：刷新按钮 + 文案。常态是「N 分钟前」（数据几时读的），
    /// 悬停到某个环时整行换成那一项的重置时刻（Todo #14）。
    ///
    /// 刷新按钮放在**左边**是为了不抖：文案长度在「刚刚」和「Claude Code 5h ·
    /// 16:39 重置」之间来回跳，按钮跟在后面就会横向乱窜；钉在行首它就一动不动。
    ///
    /// 文案是相对 now 的，包 `TimelineView(.everyMinute)` 才会随时间老化 ——
    /// 否则数据不变时这行会冻在第一次渲染的值上（#540 同款坑）。
    private var freshnessRow: some View {
        HStack(spacing: 7) {
            Button {
                if !reduceMotion {
                    withAnimation(.easeInOut(duration: 0.7)) { refreshSpin += 360 }
                }
                Task { await quota.refresh() }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 9.5, weight: .semibold))
                    .rotationEffect(.degrees(refreshSpin))
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.tertiary)
            .disabled(quota.refreshing)
            .help("刷新额度")
            .accessibilityLabel("刷新额度")

            TimelineView(.everyMinute) { ctx in
                Text(QuotaRingLayout.footnote(
                    hovered: hoveredRing,
                    fetchedAt: [quota.claude?.fetchedAt, quota.codex?.fetchedAt],
                    now: ctx.date) ?? "—")
                    .font(.system(size: 10.5).monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
        }
        // 行高按最高的那个元素定死：文案在两种态之间切换时行不该长高/变矮。
        .frame(height: 14)
        .padding(.top, 4)
    }
}

// MARK: - 单个环

private struct QuotaRingView: View {
    let ring: QuotaRing
    /// 由页脚统一持有（环下那行要显示悬停项的重置时刻，得知道是哪一个）。
    @Binding var hoveredRingID: String?

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var hovering: Bool { hoveredRingID == ring.id }

    private static let height: CGFloat = 33
    private static let circleWidth: CGFloat = 33
    /// 长条比正圆宽 —— 模型名（Fable / Opus 4.7）在正圆里挤不开。
    private static let stadiumWidth: CGFloat = 54
    private static let lineWidth: CGFloat = 3

    private var width: CGFloat {
        ring.form == .stadium ? Self.stadiumWidth : Self.circleWidth
    }
    /// 长条那格字多一点空间，字号给大半档。
    private var captionSize: CGFloat { ring.form == .stadium ? 10 : 9.5 }
    private var percentSize: CGFloat { ring.form == .stadium ? 9.5 : 9 }

    private var color: Color { CrewSidebarView.quotaColor(level: ring.level) }

    var body: some View {
        ZStack {
            track
            bar
            caption
            percent
        }
        .frame(width: width, height: Self.height)
        .contentShape(Rectangle())
        .onHover { inside in
            // 只认自己进/出：鼠标从 A 滑到 B 时 B 的 enter 可能先于 A 的 exit 到，
            // 无条件写 nil 会把刚点亮的 B 抹掉。
            let next: String? = inside ? ring.id : (hoveredRingID == ring.id ? nil : hoveredRingID)
            if reduceMotion {
                hoveredRingID = next
            } else {
                withAnimation(.easeInOut(duration: 0.1)) { hoveredRingID = next }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(ring.windowLabel) 已用 \(ring.usedPercent)%")
    }

    // 环形几何：`.padding(lineWidth/2)` 把描边压回外框内（描边是居中画的，
    // 不内缩会有一半溢出 33pt 的框，跟相邻的环贴上）。
    @ViewBuilder
    private var track: some View {
        switch ring.form {
        case .circle:
            Circle()
                .stroke(Theme.Palette.quotaTrack, lineWidth: Self.lineWidth)
                .padding(Self.lineWidth / 2)
        case .stadium:
            StadiumRingShape()
                .stroke(Theme.Palette.quotaTrack, lineWidth: Self.lineWidth)
                .padding(Self.lineWidth / 2)
        }
    }

    @ViewBuilder
    private var bar: some View {
        let style = StrokeStyle(lineWidth: Self.lineWidth, lineCap: .round)
        switch ring.form {
        case .circle:
            // Circle 的 trim 从 3 点方向起步，转 -90° 让它从正上方开始。
            Circle()
                .trim(from: 0, to: ring.progress)
                .stroke(color, style: style)
                .rotationEffect(.degrees(-90))
                .padding(Self.lineWidth / 2)
        case .stadium:
            StadiumRingShape()
                .trim(from: 0, to: ring.progress)
                .stroke(color, style: style)
                .padding(Self.lineWidth / 2)
        }
    }

    /// 平时显示的窗名 / 模型名。
    private var caption: some View {
        Text(ring.caption)
            .font(.system(size: captionSize, weight: .semibold))
            .foregroundStyle(Theme.Palette.ink)
            .lineLimit(1)
            .minimumScaleFactor(0.75)
            .padding(.horizontal, 4)
            .opacity(hovering ? 0 : 1)
    }

    /// 悬停时显示的百分比。数字等宽（换档时不跳字），% 号再小一号并压淡 ——
    /// 主角是数字，单位不该抢戏。**不斜体**（人类明确要求）。
    private var percent: some View {
        (Text("\(ring.usedPercent)")
            .font(.system(size: percentSize, weight: .semibold).monospacedDigit())
            .foregroundStyle(Theme.Palette.ink)
         + Text("%")
            .font(.system(size: percentSize * 0.72, weight: .semibold))
            .foregroundStyle(Theme.Palette.ink.opacity(0.62)))
            .lineLimit(1)
            .opacity(hovering ? 1 : 0)
    }
}

// MARK: - 长条环形

/// 长条（stadium）环形。自己拼 path 而不是用系统 `Capsule()`：要让进度
/// **从正上方中点顺时针**起步，跟正圆环对齐；系统 Capsule 的 path 起点没有
/// 公开契约，`trim` 出来的起点会飘（不同 SDK 版本可能不一样）。
struct StadiumRingShape: Shape {
    func path(in rect: CGRect) -> Path {
        let r = min(rect.height, rect.width) / 2
        var p = Path()
        p.move(to: CGPoint(x: rect.midX, y: rect.minY))          // 正上方中点
        p.addLine(to: CGPoint(x: rect.maxX - r, y: rect.minY))   // 上边向右
        p.addArc(center: CGPoint(x: rect.maxX - r, y: rect.midY), radius: r,
                 startAngle: .degrees(-90), endAngle: .degrees(90), clockwise: false)
        p.addLine(to: CGPoint(x: rect.minX + r, y: rect.maxY))   // 下边向左
        p.addArc(center: CGPoint(x: rect.minX + r, y: rect.midY), radius: r,
                 startAngle: .degrees(90), endAngle: .degrees(270), clockwise: false)
        p.closeSubpath()
        return p
    }
}
#endif
