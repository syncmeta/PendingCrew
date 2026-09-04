#if os(macOS)
import XCTest

/// **丢更新：两个进程各拿一份过期快照、各自整份覆写同一个账本**
/// （2026-09-04，翻默认引入的回归）。
///
/// ## 病历
///
/// `LocalCrewStore.persistToDisk` 是 `write(..., options: [.atomic])` **整份覆写**，
/// 没有 flock、没有版本号；而每个进程 `init` 时 `loadFromDisk` 一次，之后**长期
/// 持有内存快照**。翻默认之前写这份账的只有 GUI 一个进程（helper 子进程走
/// `LocalCrewControlStore` 那套「一条命令一个文件、排空即删」，正是为了避开这个问题）。
/// **翻默认把第二个写入方带了进来**：编排搬进 daemon 之后，
/// `recordSessionMember` 每次 session 生命周期变动都写，而人在界面上建 crew、
/// 挂/摘父边、隐藏、改名走的是 GUI 进程 —— 两者在正常使用中**天天重叠**。
///
/// 真 daemon 上复现过（机长，隔离数据根）：外部写入把标题改了、隐藏也设了，
/// daemon 随后写一次盘，**两处改动全没了，一声不吭**。丢的可能不是一个标题，
/// 而是一条父子边或一个刚建的 crew。
///
/// ## 这一组断的是**结果**，不是实现
///
/// 每条测试问的都是「**两次更新是不是都还在**」，不是「有没有加锁」——
/// 锁只是当前的修法，而丢更新是不许发生的事实。修法换了，这些测试不该跟着改。
///
/// ⚠️ **边界（不许含糊）**：这里的两个 store 实例在**同一个进程**里，
/// 不等于两个真进程。它们共用一个 `flock` 的语义（flock 对同进程不同 fd 也互斥），
/// 但真进程还有调度、页缓存、崩溃时机等这里造不出来的东西。真进程那一半由机长
/// 手工复现过一次，进不了回归网；这一组才是可重复、能做负向对照的那把尺子。
/// 两者互补：这一组证明它**必然**发生，那一次证明它**真的**发生过。
@MainActor
final class LocalCrewStoreLostUpdateTests: XCTestCase {

    private var dir: URL!

    override func setUpWithError() throws {
        dir = URL(fileURLWithPath: "/tmp/pcrew-lost-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: dir)
    }

    private func makeStore() -> LocalCrewStore { LocalCrewStore(baseDirectory: dir) }

    private func makeCrew(_ store: LocalCrewStore, _ title: String) -> String {
        store.createCrew(.make(
            responsibleSubjectId: "local-byok", title: title, machineId: nil,
            workingDirectory: dir.path, captainAgentKind: "claude_code",
            initialTitleSource: .human,
            captain: .systemGenerated(templateName: nil))).crewId
    }

    /// 盘上那份现在长什么样（**绕开任何内存快照**，直接读文件）。
    private func onDisk() throws -> [String: LocalCrew] {
        let data = try Data(contentsOf: dir.appendingPathComponent("local-crews.json"))
        let file = try JSONDecoder().decode(LocalCrewFile.self, from: data)
        return Dictionary(uniqueKeysWithValues: file.crews.map { ($0.id, $0) })
    }

    // MARK: -

    /// **主条**：A 改名、B 隐藏 —— 两次更新都必须还在。
    ///
    /// 这正是真 daemon 上复现到的形状：一个写入方拿着它加载时的快照整份覆写，
    /// 把中间别人写进去的东西抹掉，**而且一声不吭**。
    func test_两个store各改一处之后两处改动都要还在() throws {
        let seed = makeStore()
        let id = makeCrew(seed, "丢更新靶子")

        // 两个写入方各自加载一次 —— 从此各持一份快照（真进程里就是 daemon 与 GUI）。
        let a = makeStore()
        let b = makeStore()

        a.setTitle(id, "人在界面上改的名字", source: .human)
        b.setManuallyHidden(id, hidden: true)

        let disk = try onDisk()
        let crew = try XCTUnwrap(disk[id])
        XCTAssertEqual(crew.title, "人在界面上改的名字",
                       "后写的那次把先写的改名抹掉了 —— 丢更新，而且一声不吭")
        XCTAssertNotNil(crew.manuallyHiddenAt, "隐藏那次更新丢了")
    }

    /// 换个顺序再来一遍 —— 「谁后写谁赢」不该因为谁先动而变。
    func test_反过来的顺序同样不许丢() throws {
        let seed = makeStore()
        let id = makeCrew(seed, "丢更新靶子")
        let a = makeStore()
        let b = makeStore()

        b.setManuallyHidden(id, hidden: true)
        a.setTitle(id, "人在界面上改的名字", source: .human)

        let crew = try XCTUnwrap(try onDisk()[id])
        XCTAssertEqual(crew.title, "人在界面上改的名字")
        XCTAssertNotNil(crew.manuallyHiddenAt, "先写的隐藏被后写的改名抹掉了")
    }

    /// **丢的可能是一整个 crew，不只是一个字段。**
    ///
    /// 人在界面上新建一个 crew（GUI 进程），后台随后因为一次 session 变动写盘
    /// （`recordSessionMember`，daemon 进程每次 session 生命周期变动都写）——
    /// 那个刚建出来的 crew 会**整个消失**。
    func test_后台写盘不许抹掉人刚新建的crew() throws {
        let seed = makeStore()
        let existing = makeCrew(seed, "已有的 crew")

        let daemon = makeStore()          // 后台：加载一次，之后长期持有快照
        let gui = makeStore()
        let fresh = makeCrew(gui, "人刚新建的 crew")

        daemon.recordSessionMember(crewId: existing, sessionId: "worker-1",
                                   displayName: "探针")

        let disk = try onDisk()
        XCTAssertNotNil(disk[fresh], "后台写一次盘就把人刚建的 crew 整个抹掉了")
        XCTAssertNotNil(disk[existing])
    }

    /// **丢的也可能是一条父子边** —— 组织结构比标题更难被人发现少了一根。
    func test_后台写盘不许抹掉人刚挂上的父边() throws {
        let seed = makeStore()
        let parent = makeCrew(seed, "父 crew")
        let child = makeCrew(seed, "子 crew")

        let daemon = makeStore()
        let gui = makeStore()
        try gui.attachParent(crewId: child, parentCrewId: parent)

        daemon.setAttention(parent, reason: "后台顺手写一次")

        let crew = try XCTUnwrap(try onDisk()[child])
        XCTAssertTrue(crew.parentCrewIds.contains(parent),
                      "后台写一次盘就把人刚挂上的父边抹掉了")
    }

    /// **防「名单式修法漏一个」**：落盘只许有一个收口。
    ///
    /// 这条不是洁癖。2026-09-04 这条 bug 的排查里，按方法名 grep 出来的写入方名单
    /// 当场就漏了一大半（说是 2 个，真实是 12 个），是靠反推落盘点才补齐的。
    /// **名单会过期，收口不会** —— 只要所有改动都从同一个口出去，将来加第 13 个
    /// 方法的人不需要知道这段历史也不会漏。
    func test_落盘只许有一个收口() throws {
        let source = try String(
            contentsOf: URL(fileURLWithPath: #filePath)
                .deletingLastPathComponent().deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("Sources/Stores/LocalCrewStore.swift"),
            encoding: .utf8)
        var offenders: [String] = []
        for (index, line) in source.split(separator: "\n", omittingEmptySubsequences: false)
            .enumerated() {
            let text = String(line)
            guard text.contains("persistToDisk") else { continue }
            // 允许：函数自己的声明、以及收口里的那一次调用（注释里提到不算）。
            let trimmed = text.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("//") || trimmed.hasPrefix("///") { continue }
            if trimmed.contains("private func persistToDisk") { continue }
            if trimmed.contains("func mutatingCrews") { continue }
            offenders.append("\(index + 1): \(trimmed)")
        }
        XCTAssertLessThanOrEqual(
            offenders.count, 1,
            """
            落盘散在多处 —— 加第 13 个方法的人一定会漏掉其中一个，而漏掉的那个就是
            下一次丢更新的入口。全部收进「锁内重读 → 改 → 写」那一个口。
            当前散落处：
            \(offenders.joined(separator: "\n"))
            """)
    }
}
#endif
