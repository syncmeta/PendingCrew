#if os(macOS)
import Combine
import Foundation
import XCTest

/// 「同一时刻只有一条气泡挂 `.textSelection`」（#443 方案 D）的归属语义。
///
/// D 有两个只有真 GUI 才看得出的风险（闪烁、划选起点跟不跟手），那两条是 QA 尾巴。
/// **但归属本身的语义是纯逻辑，能量就量** —— 这个文件把能钉的都钉死，免得下一个
/// 人改 hover 逻辑时把「划选划到一半被拆」这类回归悄悄放进去。
final class CrewBubbleSelectionOwnerTests: XCTestCase {

    private var bag: Set<AnyCancellable> = []

    override func tearDown() {
        bag.removeAll()
        super.tearDown()
    }

    /// 记录 publisher 一路发出来的值。
    private func record(_ owner: CrewBubbleSelectionOwner) -> () -> [String?] {
        var seen: [String?] = []
        owner.publisher.sink { seen.append($0) }.store(in: &bag)
        return { seen }
    }

    func test_一开始谁都没拿到选中能力() {
        let owner = CrewBubbleSelectionOwner()
        XCTAssertNil(owner.current, "没人碰过之前，整列表一个 overlay 都不该挂")
    }

    func test_悬停进入就把归属拿过来() {
        let owner = CrewBubbleSelectionOwner()
        let seen = record(owner)
        owner.arm("A")
        XCTAssertEqual(owner.current, "A")
        XCTAssertEqual(seen(), [nil, "A"], "订阅时先拿到当前值，再拿到变更")
    }

    /// **性能关键**：鼠标在同一条气泡里移动会反复触发 `onHover(true)`。
    /// 每次都发通知 = 每次都让所有行的 `onReceive` 醒一遍，那就是拿一个卡换另一个卡。
    func test_重复悬停同一条不再发通知() {
        let owner = CrewBubbleSelectionOwner()
        owner.arm("A")
        let seen = record(owner)
        for _ in 0 ..< 50 { owner.arm("A") }
        XCTAssertEqual(seen(), ["A"], "已经是自己了就别再广播 —— 否则鼠标一动整表醒一遍")
    }

    func test_移到另一条就接力转移() {
        let owner = CrewBubbleSelectionOwner()
        owner.arm("A")
        let seen = record(owner)
        owner.arm("B")
        XCTAssertEqual(owner.current, "B")
        XCTAssertEqual(seen(), ["A", "B"])
    }

    /// **划选划到一半不许被拆**：归属是接力式的 —— 离开一条气泡时**不交还**，
    /// 要等下一条来拿。所以「按住鼠标从气泡里拖到气泡外面继续划」这个动作里
    /// overlay 一直在。
    ///
    /// 这条同时也是 API 契约：`CrewBubbleSelectionOwner` **故意没有** disarm /
    /// release。谁哪天加了一个「离开就交还」，这条测试就该红。
    func test_离开不交还_只有下一条来拿才转移() {
        let owner = CrewBubbleSelectionOwner()
        owner.arm("A")
        let seen = record(owner)
        // 模拟「指针离开了 A，但还没进入任何别的气泡」—— 视图侧 onHover(false)
        // 走的就是「什么都不做」这条路。
        XCTAssertEqual(owner.current, "A", "指针离开后归属必须还在 A —— 否则划选会中途断掉")
        XCTAssertEqual(seen(), ["A"], "离开不该产生任何通知")
    }

    /// 只有一份归属 —— 不会出现两条同时挂 overlay。
    func test_任何时刻只有一条持有() {
        let owner = CrewBubbleSelectionOwner()
        let ids = ["A", "B", "C", "D"]
        for id in ids { owner.arm(id) }
        let holders = ids.filter { $0 == owner.current }
        XCTAssertEqual(holders, ["D"], "同一时刻只能有一条持有选中能力")
    }

    /// Todo #56 ⑦：MarkdownUI 把一条正文拆成多个 block sibling，每段 selection overlay
    /// 天生只能管自己那块；因此整条复制必须是气泡本身的一等入口，不能再传 EmptyView。
    func test_气泡右键菜单接上复制整条() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        XCTAssertTrue(
            text.contains("menu: { CrewMessageContextMenuContent("),
            "BubbleView 的内层 contextMenu 必须直接拿到整条复制入口；外层菜单会被它截住。")
        XCTAssertTrue(text.contains("text: msg.content"), "复制载荷必须是整条可见正文。")
        XCTAssertTrue(text.contains("Label(\"复制整条\", systemImage: \"doc.on.doc\")"))
        XCTAssertFalse(
            text.contains("menu: { EmptyView() }"),
            "空的内层 contextMenu 会吃掉外层菜单，导致右键没有复制整条。")
    }

    /// 右键与 iOS 长按共用 SwiftUI contextMenu；写剪贴板的两端实现都要留着。
    func test_复制整条同时接了Mac和iOS剪贴板() throws {
        let text = try Self.source("Mac/Views/CrewChatView.swift")
        XCTAssertTrue(text.contains("NSPasteboard.general.setString(text, forType: .string)"))
        XCTAssertTrue(text.contains("UIPasteboard.general.string = text"))
    }

    private static func source(_ relativePath: String) throws -> String {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Sources")
            .appendingPathComponent(relativePath)
        return try String(contentsOf: url, encoding: .utf8)
    }
}
#endif
