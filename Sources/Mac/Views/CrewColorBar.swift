#if os(macOS)
import SwiftUI

/// crew 竖色条 —— 侧栏"头像"位的身份标识（取代 emoji 群像）。
///
/// 递归二分构造：本 crew 的颜色占**上半段**，**下半段**整体照搬父 crew 的
/// 色条（等比压成一半）。于是 DAG 第 N 层的 crew 恰好有 N 段颜色：
/// 自身 1/2、父 1/4、祖父 1/8……最末两段等高。一眼读出"它是谁 + 它挂在
/// 谁的谱系下"。
struct CrewColorBar: View {
    /// 色链：[0] = 本 crew，依次向上直到根。至少 1 个。
    let colors: [Color]
    var width: CGFloat = 5
    var height: CGFloat = 32

    var body: some View {
        VStack(spacing: 0) {
            ForEach(0..<max(1, colors.count), id: \.self) { i in
                (colors.indices.contains(i) ? colors[i] : .gray)
                    .frame(height: segmentHeights[i])
            }
        }
        .frame(width: width, height: height)
        .clipShape(RoundedRectangle(cornerRadius: width / 2, style: .continuous))
    }

    /// 逐段折半，末段吃掉全部余量（与倒数第二段等高），保证总高恰为 height。
    private var segmentHeights: [CGFloat] {
        let n = max(1, colors.count)
        var out: [CGFloat] = []
        var remaining = height
        for i in 0..<n {
            if i == n - 1 {
                out.append(remaining)
            } else {
                let h = remaining / 2
                out.append(h)
                remaining -= h
            }
        }
        return out
    }

    // MARK: - 取色 / 色链

    /// 由 crew.id 确定性取色 —— FNV-1a 哈希 → 色相，饱和/明度固定。
    /// 同 id 永远同色（与旧 emoji 头像同一播种思路），浅/深色模式通用。
    static func color(forCrewId id: String) -> Color {
        var h: UInt64 = 1469598103934665603 // FNV-1a offset
        for s in id.unicodeScalars {
            h ^= UInt64(s.value)
            h = h &* 1099511628211
        }
        let hue = Double(h % 360) / 360
        return Color(hue: hue, saturation: 0.62, brightness: 0.82)
    }

    /// 沿「第一父」边向根收集色链（本 crew 在前）。多父 crew 取
    /// `parentCrewIds` 首个（行内已有分叉图标提示多父）；父不在本组子集时
    /// 仍取其 id 的颜色，但无法继续上溯。防环 + 封顶 8 层。
    static func chain(for crew: CrewSummary, crewsById: [String: CrewSummary]) -> [Color] {
        var colors = [color(forCrewId: crew.id)]
        var visited: Set<String> = [crew.id]
        var parentId = crew.parentCrewIds.first
        while let pid = parentId, !visited.contains(pid), colors.count < 8 {
            colors.append(color(forCrewId: pid))
            visited.insert(pid)
            parentId = crewsById[pid]?.parentCrewIds.first
        }
        return colors
    }
}
#endif
