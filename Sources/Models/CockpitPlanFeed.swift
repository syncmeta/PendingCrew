import Foundation

/// 驾驶舱正文那一格的账 —— **按 crew 认领**（人类 Todo #96）。
///
/// ## 为什么需要它
///
/// 读账从「同步」改成「后台读、回主线程只赋值」之后，多了一个同步版本没有的坑：
/// 结果**会迟到**。切到 B 之后，A 那次读才回来，直接赋值就等于把 A 的作战板挂在
/// B 的标题下面 —— 而且它看起来完全正常，人不会怀疑自己看错了群。
///
/// 所以这一格记的不是「一串计划」，是「**谁的**一串计划」：
/// - `apply` 只在「这次读请求的 crew == 现在要显示的 crew」时采纳，迟到的直接丢。
/// - `plans(for:)` 只在认领的 crew 对得上时才交出行；对不上返回空，
///   宁可让人看到一瞬空白，也不给他看另一个 crew 的计划。
///
/// 纯值类型、无 IO、无并发 —— 所以这两条行为能脱离 SwiftUI 直接单测。
struct CockpitPlanFeed: Equatable {
    /// 当前这批行属于哪个 crew（nil = 还没有任何一次读被采纳）。
    private var loadedCrewId: String?
    private var rows: [CockpitPlanItem] = []

    init() {}

    /// 现在该显示给 `crewId` 的行。认领的不是它 → 空。
    func plans(for crewId: String) -> [CockpitPlanItem] {
        loadedCrewId == crewId ? rows : []
    }

    /// 采纳一次后台读的结果。
    ///
    /// - Parameters:
    ///   - requested: 发起那次读时问的是哪个 crew。
    ///   - current: 现在界面上要显示的是哪个 crew。
    /// - Returns: 是否采纳。`false` = 迟到的结果，已丢弃。
    @discardableResult
    mutating func apply(rows: [CockpitPlanItem], requested: String, current: String) -> Bool {
        guard requested == current else { return false }
        loadedCrewId = requested
        self.rows = rows
        return true
    }
}
