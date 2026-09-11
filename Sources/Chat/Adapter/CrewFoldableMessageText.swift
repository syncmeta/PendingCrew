import SwiftUI

/// 群聊气泡的**可折叠正文**（人类 Todo #104）。
///
/// 人类原话：「现在我就是希望总机长和消息折叠这两个先做好，**因为现在消息太多太乱了**」。
/// 治的是「乱」= 条数占地，所以**默认收起**；点一下展开。
///
/// 折不折、摘要是什么，全在纯判定层 `CrewMessageFold`（有单测）。这里只做两件事：
/// 拿判定结果决定渲染哪一种，以及把展开状态钉在这条消息的身份上。
///
/// **收起条上留着行数是故意的**：一堵墙折起来仍看得出是堵墙 —— 免得大家因为
/// 「反正折着」把消息写得更长。折叠让墙占地小，不该让它变免费。
///
/// **只影响眼前这块屏**：不改谁看得见什么，也不碰给 agent 的注入面（那条路
/// 压根不经过这里，有 `CrewMessageFoldTests` 钉着）。
struct CrewFoldableMessageText: View {
    let text: String
    /// 作者显式写的那一行结论（人类 Todo #143）。给了就用它当收起态标题，
    /// **不给才退回猜**（`CrewMessageFold.derivedSummary` 取第一个粗体）。
    ///
    /// 这一级 `CrewMessageFold` 早就留着（形参 + 注释「最可靠的一级」），
    /// 但在 2026-09-11 之前**没有任何调用点传过它** —— 整条路不存在。
    var headline: String? = nil
    let allowCodeRun: Bool
    let citations: [MessageCitation]
    /// 正在流式吐字的消息不折 —— 折一个还在长的东西，人会以为它写完了。
    var isStreaming: Bool = false

    @State private var expanded = false

    /// 判定整个搬去 `CrewMessageFold.decideForRender` —— **这个 View 不进 test
    /// bundle**，判定留在这里就等于那一节没有尺子。
    private var folded: CrewMessageFold.Folded? {
        CrewMessageFold.decideForRender(text: text, headline: headline, isStreaming: isStreaming)
    }

    var body: some View {
        if let folded, !expanded {
            collapsedRow(folded)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                MarkdownText(text: text, allowCodeRun: allowCodeRun, citations: citations)
                    .fixedSize(horizontal: false, vertical: true)
                if folded != nil {
                    collapseButton
                }
            }
        }
    }

    private func collapsedRow(_ f: CrewMessageFold.Folded) -> some View {
        Button {
            expanded = true
        } label: {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: "chevron.right")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(Theme.Palette.inkMuted)
                Text(f.summary)
                    .font(Theme.Fonts.rounded(size: 15, weight: .semibold))
                    .foregroundStyle(Theme.Palette.ink)
                    .multilineTextAlignment(.leading)
                    .lineLimit(2)
                // 不放 Spacer：气泡照旧贴着内容宽度长，折起来的这条才跟周围
                // 的普通气泡是同一族，而不是一根横贯全宽的控件条。
                Text("\(f.lineCount) 行")
                    .font(Theme.Fonts.rounded(size: 11, weight: .medium))
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .layoutPriority(1)
                    .padding(.leading, 2)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private var collapseButton: some View {
        Button {
            expanded = false
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "chevron.up")
                    .font(.system(size: 9, weight: .semibold))
                Text("收起")
                    .font(Theme.Fonts.rounded(size: 11, weight: .medium))
            }
            .foregroundStyle(Theme.Palette.inkMuted)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
