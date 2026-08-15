import SwiftUI

/// 居中的相对时间 pill。仅当与上一条间隔 ≥ 5 分钟时插入（由 CrewChatView 决定）。
struct CrewTimeSeparator: View {
    let date: Date

    var body: some View {
        // label 是「相对 now」的文案而视图输入(date)不变 —— 不包 TimelineView 的话
        // 首次渲染算死的「几秒前」永远不会老化成「X 分钟前」。
        TimelineView(.everyMinute) { _ in
            Text(CrewTimeSeparator.label(date))
                .font(Theme.Fonts.caption2)
                .foregroundStyle(Theme.Palette.inkMuted)
                .padding(.vertical, 6)
                .frame(maxWidth: .infinity)
        }
    }

    /// ISO 字符串 → Date（白板 createdAt 是 ISO-8601）。
    /// 实现在 `CrewTimestamp.parse`（纯 Foundation，能进 test bundle）—— 这里只转发，
    /// 别在这儿再写一份带/不带小数秒的分支。
    static func parse(_ iso: String) -> Date? { CrewTimestamp.parse(iso) }

    /// 两条之间是否要插分隔（首条总插）。
    static func needsSeparator(prev: Date?, cur: Date) -> Bool {
        guard let prev else { return true }
        return cur.timeIntervalSince(prev) >= 300
    }

    static func label(_ d: Date) -> String {
        let cal = Calendar.current
        let now = Date()
        let secs = now.timeIntervalSince(d)
        if secs < 60 { return "几秒前" }
        if secs < 3600 { return "\(Int(secs / 60)) 分钟前" }
        let hm = DateFormatter(); hm.dateFormat = "HH:mm"
        if cal.isDateInToday(d) { return hm.string(from: d) }
        if cal.isDateInYesterday(d) { return "昨天 \(hm.string(from: d))" }
        let md = DateFormatter(); md.dateFormat = "MM/dd HH:mm"
        return md.string(from: d)
    }
}
