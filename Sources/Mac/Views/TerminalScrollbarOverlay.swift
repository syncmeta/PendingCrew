#if os(macOS)
import SwiftUI

/// 外置 overlay 竖向滚动条：叠在终端右侧那条 SwiftTerm 预留的空当里（~15pt，网格宽度已在
/// `getEffectiveWidth` 扣掉，所以不压最右列字符）。macOS overlay 语义 —— 用户滚轮/拖动时淡入，
/// 悬停时保持，停手约 1.5s 后淡出。
///
/// 几何与信号全部来自 `AgentTerminalSession.scrollState`（取自 SwiftTerm 公开滚动接口
/// canScroll/scrollPosition/scrollThumbsize）；拖动/点击回调走 `session.scrollTerminal(toPosition:)`
/// 驱动 SwiftTerm `scroll(toPosition:)`。SwiftTerm 内部那条焊死的 NSScroller 已被
/// `ActivityTerminalView` 藏掉，这里是它的替身。
struct TerminalScrollbarOverlay: View {
    @ObservedObject var session: AgentTerminalSession

    /// 轨道宽度对齐 SwiftTerm 预留的 scrollerWidth(~15pt)；knob 更窄、居中，不顶边。
    private let trackWidth: CGFloat = 15
    private let knobWidth: CGFloat = 6
    private let knobInset: CGFloat = 3
    private let minKnobHeight: CGFloat = 28
    private let autoHideDelay: TimeInterval = 1.5

    @State private var visible = false
    @State private var hovering = false
    @State private var dragging = false
    @State private var hideWork: DispatchWorkItem?

    var body: some View {
        GeometryReader { geo in
            let s = session.scrollState
            // 几何换算收口到纯函数（Tests/PendingCrewTests/TerminalScrollbarGeometryTests）。
            let g = TerminalScrollbarGeometry(
                trackExtent: geo.size.height, inset: knobInset, minKnobExtent: minKnobHeight)
            let knobHeight = g.knobExtent(thumbSize: s.thumbSize)
            let knobY = g.knobOffset(position: s.position, thumbSize: s.thumbSize)

            ZStack(alignment: .top) {
                Color.clear                                    // 填满轨道，供命中测试
                if s.canScroll {
                    RoundedRectangle(cornerRadius: knobWidth / 2, style: .continuous)
                        .fill(Color.primary.opacity(dragging ? 0.55 : (hovering ? 0.45 : 0.32)))
                        .frame(width: knobWidth, height: knobHeight)
                        .offset(y: knobY)
                }
            }
            .frame(width: trackWidth, height: geo.size.height)
            .contentShape(Rectangle())
            .gesture(
                // 抓轨道任意处：把 knob 中心对到光标，既支持拖动跟随也支持点击跳转。
                DragGesture(minimumDistance: 0)
                    .onChanged { value in
                        dragging = true
                        reveal()
                        session.scrollTerminal(
                            toPosition: g.position(forPointerAt: value.location.y, thumbSize: s.thumbSize))
                    }
                    .onEnded { _ in
                        dragging = false
                        scheduleHide()
                    }
            )
            .opacity(visible ? 1 : 0)
            .animation(.easeInOut(duration: 0.18), value: visible)
            .onHover { h in
                hovering = h
                if h { reveal() } else { scheduleHide() }
            }
            // 用户主动滚动（滚轮/拖动）→ 淡入并重置隐藏计时；输出自动滚屏不 bump，不触发。
            .onChange(of: s.userScrollTick) { _ in
                reveal()
                scheduleHide()
            }
            // 不可滚（内容没超一屏 / alternate buffer）→ 立即隐藏。
            .onChange(of: s.canScroll) { can in
                if !can { hideWork?.cancel(); visible = false }
            }
        }
        .frame(width: trackWidth)
        // 不可滚时整条不吃点击，别在空当里白截终端的鼠标事件。
        .allowsHitTesting(session.scrollState.canScroll)
    }

    private func reveal() {
        hideWork?.cancel()
        visible = true
    }

    /// 停手/移出后延迟淡出；拖动中或悬停中不排隐藏（那两个状态自身结束时会再排一次）。
    private func scheduleHide() {
        hideWork?.cancel()
        guard !dragging, !hovering else { return }
        let work = DispatchWorkItem { visible = false }
        hideWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + autoHideDelay, execute: work)
    }
}
#endif
