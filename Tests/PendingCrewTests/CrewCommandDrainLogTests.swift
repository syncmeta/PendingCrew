#if os(macOS)
import XCTest

/// **排空一批机长命令时留一行日志**（2026-09-04）。
///
/// ## 它不是用来抓某个 bug 的，是用来划边界的
///
/// 今天有一次「投 2 条命令、见到 3 个 session」的异常，判定它花了**三趟受控实验**：
/// 命令文件**排空即删**，daemon 日志只记启动 / 连接 / 退出、**不记排空** ——
/// 等人发现异常时，能判定它的东西已经不存在了。当时唯一站得住的结论是「查不了」。
///
/// 真相后来查明是**消费方**把第一条处理了两遍（`@Published` 数组当队列用）。
/// **如果当时就有这行日志，它会显示「一次排空、两个不同文件名」——当场把嫌疑从
/// 命令通道摘出去、直接指向消费方。** 所以这行日志的作用是**把「通道」和「消费」
/// 这两段分开**，不是「抓重复排空」。
///
/// ## 为什么必须带文件名
///
/// 两个候选解释在日志里长得不一样：**「多写了一个文件」是两个不同文件名，
/// 「同一个文件被排空两次」是同一个文件名出现两次**。只记命令 id 的话，
/// 如果重复写入恰好用了不同 uuid，两种解释**仍然分不开** —— 那这行日志就白记了。
final class CrewCommandDrainLogTests: XCTestCase {

    private var dir: URL!
    private var lines: [String] = []

    override func setUpWithError() throws {
        dir = URL(fileURLWithPath: "/tmp/pcrew-drainlog-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        lines = []
        CrewCommandDrainLog.sink = { [weak self] line in self?.lines.append(line) }
    }

    override func tearDownWithError() throws {
        CrewCommandDrainLog.sink = CrewCommandDrainLog.defaultSink
        try? FileManager.default.removeItem(at: dir)
    }

    /// 写一个命令文件；返回文件名。
    @discardableResult
    private func writeCommand(id: String, kind: String, crewId: String) throws -> String {
        let name = "\(crewId).\(id).crewcmd.json"
        let payload: [String: Any] = [
            "id": id, "crewId": crewId, "kind": kind, "brief": "随便",
            "ts": "2026-09-04T09:00:00Z",
        ]
        try JSONSerialization.data(withJSONObject: payload)
            .write(to: dir.appendingPathComponent(name))
        return name
    }

    // MARK: -

    /// **主条**：排空两条 → **一行**日志，里面有条数，也有**两个文件名**。
    func test_排空一批留一行日志且带条数与每条文件名() throws {
        let store = LocalCrewControlStore(directory: dir)
        let a = try writeCommand(id: "AAAA-1111", kind: "start_session", crewId: "local-x")
        let b = try writeCommand(id: "BBBB-2222", kind: "start_session", crewId: "local-x")

        let cmds = store.drainCommands()
        XCTAssertEqual(cmds.count, 2, "前置条件：两条都排出来了")

        XCTAssertEqual(lines.count, 1, "一次排空一行 —— 每条一行的话就数不清「这一批」是几条")
        let line = try XCTUnwrap(lines.first)
        XCTAssertTrue(line.contains("2"), "要带这一批的条数：\(line)")
        XCTAssertTrue(line.contains(a), "缺文件名 \(a)，两种解释就分不开：\(line)")
        XCTAssertTrue(line.contains(b), "缺文件名 \(b)：\(line)")
        XCTAssertTrue(line.contains("start_session"), "要带 kind：\(line)")
        XCTAssertTrue(line.contains("local-x"), "要带 crewId：\(line)")
    }

    /// **同一个文件名出现两次**（真被排空两遍）与**两个不同文件名**（多写了一个）
    /// 在日志里必须长得不一样 —— 这正是这行日志存在的全部理由。
    func test_同名重复与不同文件在日志里分得开() throws {
        let store = LocalCrewControlStore(directory: dir)
        let a = try writeCommand(id: "AAAA-1111", kind: "start_session", crewId: "local-x")
        let b = try writeCommand(id: "BBBB-2222", kind: "start_session", crewId: "local-x")
        _ = store.drainCommands()
        let twoDifferentFiles = try XCTUnwrap(lines.first)

        lines = []
        // 同一个文件名被排空两次 = 两行日志、同一个文件名。
        try writeCommand(id: "AAAA-1111", kind: "start_session", crewId: "local-x")
        _ = store.drainCommands()
        try writeCommand(id: "AAAA-1111", kind: "start_session", crewId: "local-x")
        _ = store.drainCommands()

        XCTAssertEqual(lines.count, 2)
        XCTAssertTrue(lines.allSatisfy { $0.contains(a) })
        XCTAssertFalse(lines.contains { $0.contains(b) })
        XCTAssertTrue(twoDifferentFiles.contains(a) && twoDifferentFiles.contains(b),
                      "「一次排空两个不同文件」这一行里两个文件名都要在")
    }

    /// **空排空不写日志** —— 目录监听每个 tick 都会调一次排空，每次都写一行会把
    /// 日志淹掉，而淹掉的日志和没有日志是同一个东西。
    func test_没东西可排时不写日志() {
        let store = LocalCrewControlStore(directory: dir)
        XCTAssertTrue(store.drainCommands().isEmpty)
        XCTAssertTrue(lines.isEmpty, "空排空也写日志的话，真正那一行会被淹在噪音里")
    }

    /// 文案是纯函数，单独钉一下形状（省得只能靠上面几条间接确认）。
    func test_文案形状() {
        let line = CrewCommandDrainLog.line(entries: [
            .init(fileName: "local-x.AAAA.crewcmd.json", id: "AAAA",
                  kind: "start_session", crewId: "local-x"),
            .init(fileName: "local-x.BBBB.crewcmd.json", id: "BBBB",
                  kind: "nudge_session", crewId: "local-x"),
        ])
        XCTAssertTrue(line.hasPrefix("排空机长命令 2 条"), line)
        XCTAssertTrue(line.contains("local-x.AAAA.crewcmd.json"), line)
        XCTAssertTrue(line.contains("nudge_session"), line)
    }
}
#endif
