#if os(macOS)
import SwiftUI

/// 侧栏「总机长」视图**顶上那个固定入口** —— 总机组那一层的群聊（人类 Todo #130 / #137）。
///
/// ## 为什么它不是一行 crew
/// 总机组是产品内建的**一层**，不是一个工作机组：它不进 crew 列表、不算进
/// 「全机 N 个 crew」、也不在组织树里（口径写在 `LocalCrewStore.listCrews`）。
/// 那道排除是对的，但它**把入口一起排掉了** —— 群聊在磁盘上开得出来，界面上
/// 却没有任何地方点得进去。
///
/// 所以这一行刻意长得**跟 crew 行不一样**：没有谱系竖色条、没有展开三角、
/// 不能拖拽改隶属、不进排序。它就是「这一层的门」，钉在最上面不动。
///
/// ## 位置是我们选的
/// 放在第三视图（「总机长」）顶上，而不是层级/时间流里：那两个视图讲的是
/// 「这台机器上有哪些机组、怎么挂的」，把一层塞进去会重新制造它本来要避开的
/// 那个误解。第三视图讲的是「现在该管什么」，总机组正是管这件事的那一层。
struct CrewChiefLayerEntryRow: View {
    let crew: CrewSummary
    @EnvironmentObject private var crewStore: CrewStore

    private var isSelected: Bool { crewStore.selectedCrewId == crew.id }

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "building.columns")
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(isSelected ? Theme.Palette.accent : Theme.Palette.inkMuted)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 1) {
                Text(crew.title)
                    .font(Theme.Fonts.bodyEmphasized)
                    .foregroundStyle(Theme.Palette.ink)
                    .lineLimit(1)
                Text("这一层的群聊 · 不是一个机组")
                    .font(Theme.Fonts.caption2)
                    .foregroundStyle(Theme.Palette.inkMuted.opacity(0.75))
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(
            RoundedRectangle(cornerRadius: Theme.Metrics.cardRadius, style: .continuous)
                .fill(isSelected ? Theme.Palette.accentBg : Color.clear)
        )
        .contentShape(Rectangle())
        .onTapGesture { crewStore.selectCrew(crew.id) }
        .help("总机组：整台机器这一层自己的群聊。它不是一个工作机组，所以不出现在下面的列表里。")
    }
}
#endif
