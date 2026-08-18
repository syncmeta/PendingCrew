#if os(macOS)
import AppKit
import XCTest
import SwiftTerm

/// 「停掉的 session 不释放内存」的判定钉子 + **可复现的前后实测**
/// （2026-08-18：开久了卡第二条）。
///
/// 病根复盘见 `TerminatedScrollbackPlan` 的类型注释。这里回答两个问题：
/// 1. **行为有没有退化** —— 停掉的 session 仍要能翻回去看输出（终端视图还在、
///    画面还在、还能滚），只是历史短了一截；短到多少由纯函数说了算，不是拍脑袋。
/// 2. **到底省了多少** —— 起 N 个终端、把栏拉宽（现场必然发生的那一下，也正是
///    SwiftTerm 把 10000 个槽位全实例化的那一下）、量 physical footprint；
///    然后走终止路径，再量一次。
final class TerminatedScrollbackPlanTests: XCTestCase {

    func test_还没滚出过一屏就只留下限() {
        XCTAssertEqual(
            TerminatedScrollbackPlan.retainedLines(rows: 40, thumbSize: 1, canScroll: false),
            TerminatedScrollbackPlan.floor)
    }

    func test_按实际行数收窄_并留折行余量() {
        // thumb = rows/总行数 → 40/0.08 = 500 行，×2 折行余量 = 1000。
        XCTAssertEqual(
            TerminatedScrollbackPlan.retainedLines(rows: 40, thumbSize: 0.08, canScroll: true),
            1_000)
    }

    func test_行数很多时夹在上限() {
        // 40/0.02 = 2000 行，×2 = 4000 → 夹到 cap。
        XCTAssertEqual(
            TerminatedScrollbackPlan.retainedLines(rows: 40, thumbSize: 0.02, canScroll: true),
            TerminatedScrollbackPlan.cap)
    }

    func test_行数很少时不低于下限() {
        // 40/0.5 = 80 行，×2 = 160 → 抬到 floor。
        XCTAssertEqual(
            TerminatedScrollbackPlan.retainedLines(rows: 40, thumbSize: 0.5, canScroll: true),
            TerminatedScrollbackPlan.floor)
    }

    /// thumb 触底（SwiftTerm 的 `max(..., 0.01)`）= 反推不准 → **不猜**，按上限保守留。
    /// 宁可少省点内存，也不许把还在的历史裁掉。
    func test_反推不可信时按上限保守保留() {
        XCTAssertEqual(
            TerminatedScrollbackPlan.retainedLines(rows: 40, thumbSize: 0.01, canScroll: true),
            TerminatedScrollbackPlan.cap, "thumb 触底")
        XCTAssertEqual(
            TerminatedScrollbackPlan.retainedLines(rows: 40, thumbSize: 0, canScroll: true),
            TerminatedScrollbackPlan.cap, "alternate buffer 时 thumb 恒 0")
        XCTAssertEqual(
            TerminatedScrollbackPlan.retainedLines(rows: 0, thumbSize: 0.5, canScroll: true),
            TerminatedScrollbackPlan.cap, "还没布局过")
    }
}

/// 端到端：真起终端视图、真喂输出、真拉宽列数，量 physical footprint 的前后差。
@MainActor
final class TerminatedScrollbackMemoryTests: XCTestCase {

    /// 本进程 physical footprint（与活动监视器「内存」列同源）。
    private static func physFootprintBytes() -> Double {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(
            MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        return kr == KERN_SUCCESS ? Double(info.phys_footprint) : 0
    }

    /// 本进程 malloc 当前**真正在用**的字节数（全部 zone 合计）。
    ///
    /// 断言用这把尺子而不是 footprint：`free()` 一发生它立刻降下来，而 physical
    /// footprint 要等 malloc 把空闲页还给系统，还不还得看这一刻堆的碎片情况 ——
    /// 单跑这个用例时它掉得干干净净，跟在整套测试后面跑就可能一点都不掉，
    /// 于是断言的成败取决于**测试执行顺序**。真实占用降没降是确定的事实，
    /// 不该用一个不确定的口径去钉。footprint 仍然量、仍然打印（那是人在活动监视器
    /// 里看到的数），只是不拿它做断言。
    private static func mallocInUseBytes() -> Double {
        var stats = malloc_statistics_t()
        malloc_zone_statistics(nil, &stats)
        return Double(stats.size_in_use)
    }

    /// 量一次：先请 malloc 把空闲页还给系统（让 footprint 尽量贴近真实占用），
    /// 再同时取两把尺子。
    private static func measure() -> (inUseMB: Double, footprintMB: Double) {
        malloc_zone_pressure_relief(nil, 0)
        return (mallocInUseBytes() / 1_048_576, physFootprintBytes() / 1_048_576)
    }

    private func makeTerminal(cols: CGFloat, rows: CGFloat) -> ActivityTerminalView {
        let view = ActivityTerminalView(frame: NSRect(x: 0, y: 0, width: cols, height: rows))
        // 与线上一致的紧凑等宽字体（`AgentTerminalView.makeNSView`）——
        // 字号决定列数，列数决定每行 `cols × 24 B`，量的必须是同一把尺子。
        view.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        return view
    }

    /// 喂 N 行输出，再把栏拉宽（现场必然发生的那一下：session 起在窄的 inspector 栏里，
    /// 用户拖宽窗口 → 列数变大 → 每一行都按新列宽重排）。
    ///
    /// 内存口径：`CharData` stride 24 B，**每行占 `cols × 24 B`，按实际行数算**
    /// （SwiftTerm 1.18 的 `Buffer.resize` 只遍历 `lines.count`，空槽位是按需创建的
    /// —— 1.13 那版遍历 `maxLength` 把整个容量都实例化，注释里那句「不是按需增长」
    /// 说的是老版本，现在不成立了）。所以一个 session 的占用 =
    /// `min(实际输出行数, scrollback) × cols × 24 B`：跑得越久越贵，顶到 10000 行封顶。
    private func fill(_ view: ActivityTerminalView, lines: Int) {
        var text = ""
        text.reserveCapacity(lines * 72)
        for i in 0..<lines {
            text += "[\(i)] agent output line with some payload so the row is not blank\r\n"
        }
        view.feed(text: text)
        view.setFrameSize(NSSize(width: 1_400, height: 500))   // ← 拉宽：每行按新列宽重排
    }

    /// 跑久了的 session（回滚已经顶满）终止后，必须把大头放掉，且终端仍可回看。
    ///
    /// 为什么夹具喂 11000 行：这就是「开一天」的稳态 —— claude 的 TUI 持续重绘，
    /// 500 行上限当初「跑一小会儿就顶满」正是这么来的（见 `ActivityTerminalView`
    /// 那段注释），10000 行也只是把顶满的时间拉长到小时级。**短命 session 本来就
    /// 不占多少**（占用按实际行数算），下面另有一条把这一点也钉住。
    func test_跑满回滚的session终止后释放内存() throws {
        let sessionCount = 3
        let baseline = Self.measure()

        var views: [ActivityTerminalView] = []
        for _ in 0..<sessionCount {
            let view = makeTerminal(cols: 600, rows: 500)
            fill(view, lines: 11_000)           // 顶满 10000 行回滚
            views.append(view)
        }
        let filled = Self.measure()

        var retained: [Int] = []
        for view in views { retained.append(view.collapseScrollbackAfterExit()) }
        let collapsed = Self.measure()

        let grewInUse = filled.inUseMB - baseline.inUseMB
        let freedInUse = filled.inUseMB - collapsed.inUseMB
        let grewFootprint = filled.footprintMB - baseline.footprintMB
        let freedFootprint = filled.footprintMB - collapsed.footprintMB
        print("""

        ── 已终止 session 的回滚缓冲（\(sessionCount) 个终端 / 各喂 11000 行 / 拉宽到 1400pt）──
                              malloc 在用        physical footprint
          起终端前          : \(String(format: "%7.1f", baseline.inUseMB)) MB      \
        \(String(format: "%7.1f", baseline.footprintMB)) MB
          回滚顶满 + 拉宽后 : \(String(format: "%7.1f", filled.inUseMB)) MB      \
        \(String(format: "%7.1f", filled.footprintMB)) MB
          终止并收窄回滚后  : \(String(format: "%7.1f", collapsed.inUseMB)) MB      \
        \(String(format: "%7.1f", collapsed.footprintMB)) MB
          ——
          每个 session：\(String(format: "%.1f", grewInUse / Double(sessionCount))) MB → \
        \(String(format: "%.1f", (grewInUse - freedInUse) / Double(sessionCount))) MB\
        （放掉 \(String(format: "%.0f", freedInUse / max(grewInUse, 0.001) * 100))%）
          footprint 同口径：+\(String(format: "%.1f", grewFootprint)) MB → 放掉 \
        \(String(format: "%.1f", freedFootprint)) MB\
        （footprint 归还与否看这一刻堆的碎片，不做断言）
          保留行数：\(retained)（上限 \(TerminatedScrollbackPlan.cap) / 下限 \
        \(TerminatedScrollbackPlan.floor)，原 \(ActivityTerminalView.scrollbackLines)）

        """)

        XCTAssertGreaterThan(grewInUse, 30, "夹具没造出该有的占用，测量口径要复核（量到 \(grewInUse) MB）")
        XCTAssertGreaterThan(freedInUse, grewInUse * 0.6,
                             "终止后该把大头放掉（只放掉 \(freedInUse)/\(grewInUse) MB）")
        XCTAssertEqual(retained, Array(repeating: TerminatedScrollbackPlan.cap, count: sessionCount),
                       "回滚顶满时反推不可信 → 该走保守上限那条路")
    }

    /// 反过来钉一条：只跑了一小会儿的 session **本来就不占多少**，收窄也不该把它的
    /// 历史裁掉。占用按实际行数算，不按容量算 —— 这条防的是「照着老版本 SwiftTerm
    /// 的心智模型去调参」。
    func test_短命session不被裁掉历史() throws {
        let view = makeTerminal(cols: 600, rows: 500)
        fill(view, lines: 300)
        let retained = view.collapseScrollbackAfterExit()
        XCTAssertGreaterThan(retained, 300,
                             "300 行的 session 收到 \(retained) 行 —— 把还在的历史裁掉了")
        XCTAssertLessThan(retained, ActivityTerminalView.scrollbackLines)
    }

    /// 行为不许退化：收窄之后终端**还在**、当前画面一字不差、仍然能往上翻。
    /// 「停掉的 session 还能回看它干了什么」是这条 UX 的硬约束。
    func test_收窄之后仍能回看输出() throws {
        let view = makeTerminal(cols: 600, rows: 400)
        fill(view, lines: 300)

        let before = Self.visibleText(view)
        XCTAssertTrue(before.contains("[299]"), "夹具没喂进去")
        XCTAssertTrue(view.canScroll, "300 行该是可回滚的")

        let retained = view.collapseScrollbackAfterExit()

        XCTAssertEqual(Self.visibleText(view), before, "当前画面不许因为收窄而变样")
        XCTAssertTrue(view.canScroll, "收窄后仍要能往上翻 —— 这是「回看」的全部意思")
        XCTAssertGreaterThanOrEqual(retained, TerminatedScrollbackPlan.floor)
        // 幂等：再收一次不该继续往下切。
        XCTAssertEqual(view.collapseScrollbackAfterExit(), retained)
    }

    /// 真跑一个会退出的子进程，验证「终止 → 自动收窄」这条线在 backend 上真接通了
    /// （不只是方法本身对）。
    func test_进程退出时自动收窄() async throws {
        let session = AgentTerminalSession(
            config: SessionConfig(kind: .claudeCode, initialPrompt: nil),
            executable: "/bin/echo",
            workdir: NSTemporaryDirectory(),
            env: [:])
        try await waitUntil(timeout: 8) { session.status != .running }
        XCTAssertLessThanOrEqual(
            session.terminalView.getTerminal().options.scrollback,
            TerminatedScrollbackPlan.cap,
            "进程都退了，回滚上限还挂在 \(ActivityTerminalView.scrollbackLines) 行上")
    }

    /// 轮询等条件成立（避免固定 sleep 的脆弱等待）。
    private func waitUntil(
        timeout: TimeInterval, _ condition: @MainActor () -> Bool
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        XCTFail("等待超时（\(timeout)s）：条件始终没成立")
    }

    private static func visibleText(_ view: ActivityTerminalView) -> String {
        let terminal = view.getTerminal()
        return (0..<terminal.rows)
            .compactMap { terminal.getLine(row: $0)?.translateToString(trimRight: true) }
            .joined(separator: "\n")
    }
}
#endif
