import Combine
import SwiftUI

/// 「只有一条气泡挂文本选中」的归属登记（#443 病根 3）。
///
/// ## 为什么不能所有气泡都常开 `.textSelection(.enabled)`
///
/// 2026-08-11 从人类机器上那份 **0.1.7 卡 68.68 秒**的 hang 报告里读到，主线程
/// 4/11 采样是这条：
///
/// ```
/// SelectionOverlay.updateNSView(_:context:)
///   → -[NSTextField setAttributedStringValue:]
///   → -[NSControl setFont:] → -[NSTextFieldCell _invalidateEffectiveFont]
/// ```
///
/// `SelectionOverlay` 就是 SwiftUI 在 macOS 上实现 `.textSelection(.enabled)` 的
/// `NSViewRepresentable` —— **每一段可选中文字背后挂一个真的 `NSTextField`**，而且
/// 每次视图图更新都要把全部 overlay 的 `updateNSView` 跑一遍。列表里有多少段文字，
/// 这一下就付多少个 NSTextField 的钱。
///
/// ## 修法：不减功能，只把「常开」改成「谁在手底下谁开」
///
/// 鼠标悬停到哪条气泡，就把选中能力**只**给那条。人类照样能划选、能部分复制，
/// 成本却从「窗口内每一条」降到「一条」。
///
/// ## 为什么用 subject 而不是 `@Published` / 直接放 `@State`
///
/// 归属如果存在宿主视图的 `@State` 里，鼠标每划过一条气泡就写一次宿主 state ——
/// 整个 `CrewChatView` 的 body 失效、整条列表重新测量。那是**拿一个卡换另一个卡**。
/// 所以归属走一个不参与 SwiftUI 依赖追踪的 subject：发一次，只有「刚交出」和
/// 「刚拿到」那两行的局部 `@State` 真的变，其余行 `armed` 原值不变、不重绘。
///
/// ## 划选起点 / 拖出气泡外
///
/// 归属是**接力式**的：悬停进入某条 → 它拿走；离开时**不主动交还**，要等下一条来拿。
/// 所以「按下鼠标、拖到气泡外面继续划」这个动作里 overlay 一直在，选中不会中途被拆。
/// （AppKit 在按住拖动时是否仍派发 hover-exit 是未定义行为，所以这里不依赖它。）
///
/// ⚠️ **闪烁 / 划选起点是否真的跟手，只有真 GUI 看得出** —— 本轮不许起窗口验，
/// 已写成 QA 尾巴挂 #443 第 10 条。
final class CrewBubbleSelectionOwner {
    /// 当前持有选中能力的气泡 id。nil = 谁都没碰过，全列表都不挂 overlay。
    private let subject = CurrentValueSubject<String?, Never>(nil)

    var publisher: AnyPublisher<String?, Never> { subject.eraseToAnyPublisher() }

    /// 当前值 —— 行首次出现时用它定初值，免得刚滚回来的那条丢了选中能力。
    var current: String? { subject.value }

    /// 悬停进入：把归属接过来。已经是自己就什么都不做（别白发一次通知）。
    func arm(_ id: String) {
        guard subject.value != id else { return }
        subject.send(id)
    }
}

/// 给一条气泡挂「按需文本选中」。macOS 走悬停接力；iOS 没有指针，长按选中是系统
/// 惯例，保持常开（iOS 侧也没有 `SelectionOverlay`/NSTextField 这条成本）。
struct BubbleTextSelection: ViewModifier {
    let owner: CrewBubbleSelectionOwner
    let id: String

    @State private var armed = false

    func body(content: Content) -> some View {
        #if os(macOS)
        content
            .onHover { inside in if inside { owner.arm(id) } }
            .onReceive(owner.publisher) { holder in
                let next = (holder == id)
                if armed != next { armed = next }
            }
            .onAppear { armed = (owner.current == id) }
            .environment(\.crewBubbleSelectable, armed)
        #else
        content.environment(\.crewBubbleSelectable, true)
        #endif
    }
}

/// 气泡内部（`BubbleView` / `MarkdownText`）读它决定挂不挂 `.textSelection`。
/// 走 environment 而不是逐层传参：`MarkdownText` 是 vendored 文件，少改一处是一处。
/// 默认 `true` —— 群聊之外的调用点（来信正文、Todo 详情…）行为一字不变。
private struct CrewBubbleSelectableKey: EnvironmentKey {
    static let defaultValue = true
}

extension EnvironmentValues {
    var crewBubbleSelectable: Bool {
        get { self[CrewBubbleSelectableKey.self] }
        set { self[CrewBubbleSelectableKey.self] = newValue }
    }
}

/// `.textSelection(.enabled)` 与 `.textSelection(.disabled)` 是两个**不同的具体类型**
/// （`EnabledTextSelectability` / `DisabledTextSelectability`），三元表达式写不出来，
/// 只能分支。收口成一个 modifier，免得每个调用点各写一遍 `if`。
struct CrewSelectableText: ViewModifier {
    @Environment(\.crewBubbleSelectable) private var selectable

    @ViewBuilder
    func body(content: Content) -> some View {
        if selectable {
            content.textSelection(.enabled)
        } else {
            content.textSelection(.disabled)
        }
    }
}

extension View {
    /// 群聊气泡里的可选中文字：由 `crewBubbleSelectable` 环境值决定挂不挂 overlay。
    func crewSelectableText() -> some View { modifier(CrewSelectableText()) }
}
