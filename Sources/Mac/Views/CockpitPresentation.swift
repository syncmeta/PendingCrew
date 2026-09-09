#if os(macOS)
import SwiftUI

/// 驾驶舱的开关位 —— **自己一个对象，不许再挂回 `CrewSessionRunner`**（人类 Todo #96）。
///
/// ## 为什么要单独拆出来
///
/// `showingCockpit` 原本是 `CrewSessionRunner`（2600+ 行）上的一个 `@Published`。
/// SwiftUI 的 `ObservableObject` **没有属性粒度**：翻这一个 bool，会给每一个观察
/// 这个 runner 的视图发 `objectWillChange` —— 群聊 `CrewChatView`、终端与 transcript
/// `CrewSessionWindowView`、侧栏每个 crew 一行的 `CrewSidebarCrewRow`（本机 42 个）、
/// 中栏、详情面板……**开一次发一次，关一次再发一次**。这就是「打开和关闭都要很久」
/// 两个方向一样慢的来源：它压根不是「驾驶舱重」，是翻开关这个动作本身在广播。
///
/// 拆出来之后，翻这个 bool 只惊动装着它的那一层（`CockpitLayer`）和驾驶舱自己。
///
/// ## 写入口刻意**不**订阅
///
/// 中栏 toolbar 那颗「驾驶舱」按钮只需要*写*。它经 `@Environment(\.cockpitPresentation)`
/// 拿到本对象 —— `@Environment` 取一个 class 值**不会**订阅它的 `objectWillChange`
/// （只有 `@EnvironmentObject` / `@ObservedObject` / `@StateObject` 才订阅）。
/// 所以按一下不会把中栏连同底下整棵群聊一起作废。
///
/// **别把它改成 `@EnvironmentObject` 注入**：那等于把刚拆掉的广播接回来一半，
/// 而且不会有任何编译错误提醒你 —— `CockpitOpenCloseCostTests` 里那条断言就是为此立的。
final class CockpitPresentation: ObservableObject {
    @Published var isOpen = false

    /// 这次要人看的是哪条计划（人类 Todo #132/#133 的「计划 #N」胶囊）。
    ///
    /// `token` 是为了让**同一个 #N 连点两次**也算一次新请求：光比 `number` 的话，
    /// 第二次点下去值没变、`onChange` 不触发 —— 人已经在驾驶舱里翻到别处了，
    /// 再点那颗胶囊却毫无反应。同一套做法见 `CrewTodoFocus`。
    struct PlanFocus: Equatable {
        let number: Int
        let token: Int
    }

    @Published private(set) var planFocus: PlanFocus?
    private var token = 0

    func open() { isOpen = true }

    /// 开驾驶舱并落在这条计划上。**开与落点是同一个动作** —— 分成两步调用时，
    /// 「开了但没落点」是一个写得出来的中间态，而它长得就跟功能坏了一样。
    func open(planNumber: Int) {
        token += 1
        planFocus = PlanFocus(number: planNumber, token: token)
        isOpen = true
    }

    func close() { isOpen = false }
}

private struct CockpitPresentationKey: EnvironmentKey {
    /// 缺省给一个真对象而不是 optional —— 预览里按钮照样点得动，只是没人在看。
    static let defaultValue = CockpitPresentation()
}

extension EnvironmentValues {
    /// **只读句柄，不是订阅**（理由见 `CockpitPresentation` 的注释）。
    var cockpitPresentation: CockpitPresentation {
        get { self[CockpitPresentationKey.self] }
        set { self[CockpitPresentationKey.self] = newValue }
    }
}
#endif
