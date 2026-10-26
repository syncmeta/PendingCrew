#if os(macOS)
import SwiftUI

/// 机组四格标识：上两格为自身，左下为父，右下为祖父。
/// 缺少已知祖先时沿最近的已知机组颜色回退，根机组四格同色。
struct CrewColorBar: View {
    /// 色链：[0] = 自身，[1] = 已知父，[2] = 已知祖父。
    let colors: [Color]
    /// 侧栏隐藏标识时保留相同宽度，统一行标题的左缘。
    static var defaultWidth: CGFloat { side }
    private static var side: CGFloat { 12 }
    private static var spacing: CGFloat { 3 }

    var body: some View {
        let own = colors.first ?? .gray
        let parent = colors.dropFirst().first ?? own
        let grandparent = colors.dropFirst(2).first ?? parent
        VStack(spacing: Self.spacing) {
            HStack(spacing: Self.spacing) {
                tile(own)
                tile(own)
            }
            HStack(spacing: Self.spacing) {
                tile(parent)
                tile(grandparent)
            }
        }
        .frame(width: Self.side, height: Self.side)
    }

    private func tile(_ color: Color) -> some View {
        RoundedRectangle(cornerRadius: 2.25, style: .continuous)
            .fill(color)
            .frame(width: (Self.side - Self.spacing) / 2, height: (Self.side - Self.spacing) / 2)
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
        return CrewAvatarPalette.softBackground(hue: hue)
    }

    /// 层级行优先使用本行所在的真实父边；扁平行沿既有第一父关系。
    /// 只读取两代已知节点，不依赖列表位置；悬挂父边和循环不继续上溯。
    static func chain(
        for crew: CrewSummary,
        crewsById: [String: CrewSummary],
        parentId: String? = nil
    ) -> [Color] {
        var ids = [crew.id]
        let requestedParent = parentId.flatMap { crew.parentCrewIds.contains($0) ? $0 : nil }
            ?? crew.parentCrewIds.first
        if let pid = requestedParent, pid != crew.id, let parent = crewsById[pid] {
            ids.append(pid)
            if let gid = parent.parentCrewIds.first,
               !ids.contains(gid), crewsById[gid] != nil {
                ids.append(gid)
            }
        }
        return ids.map(color(forCrewId:))
    }
}
#endif
