import XCTest

/// 「目录没被信任 → 把该跑的命令原样给人」这条路。
///
/// 关键的几条都走**真文件**（tmp 下造一个假 home）：检测和渲染各自绿、中间那道缝没人量，
/// 正是这条线上栽过的形状 —— 自己把「已信任 / 未信任」当参数喂进渲染，
/// 证明的只是「渲染按字段走」，证明不了「字段是照着真文件填的」。
final class WorkdirTrustPromptTests: XCTestCase {

    private var home: URL!
    private var workdir: String!

    override func setUpWithError() throws {
        home = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("trust-prompt-" + UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        workdir = home.appendingPathComponent("CrewGround/Lisbon", isDirectory: true).path
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: home)
    }

    private func write(_ text: String, to relative: String) throws {
        let url = home.appendingPathComponent(relative)
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(text.utf8).write(to: url)
    }

    private let bothInstalled: [LocalCodingAgentKind] = [.claudeCode, .codex]

    /// 生产那条路（读真文件 → 出提示）。
    private func prompt(installed: [LocalCodingAgentKind]? = nil)
        -> WorkdirTrustPrompt.Prompt? {
        WorkdirTrustPrompt.prompt(workdir: workdir, home: home,
                                  installed: installed ?? bothInstalled)
    }

    // MARK: - 检测 → 提示（端到端，真文件）

    /// 两家都没信任过：必须出提示，而且提示里带着**真实绝对路径**和
    /// **能直接复制粘贴进 Terminal 的命令**。
    ///
    /// 这条是那条「变异自证」用的尺子：把检测改成恒返回「已信任」，它必须红。
    func test_真文件_两家都未信任_给出带真实路径的可复制命令() throws {
        try write(#"{"projects": {}}"#, to: ".claude.json")
        try write("model = \"gpt-5\"\n", to: ".codex/config.toml")

        let p = try XCTUnwrap(prompt(), "没信任过就必须提示")
        XCTAssertEqual(p.runners, [.claudeCode, .codex])
        XCTAssertEqual(p.path, workdir, "命令里出现的必须是真实绝对路径")
        XCTAssertEqual(p.commands, ["cd '\(workdir!)' && claude",
                                    "cd '\(workdir!)' && codex"])
        for line in p.commands {
            XCTAssertTrue(WorkdirTrustPrompt.body(p).contains(line), "正文里要带上这条命令")
            XCTAssertTrue(WorkdirTrustPrompt.chatMessage(p).contains(line))
        }
    }

    /// 「条目在、值是 `false`」是真踩过的形状 —— 不许当成「已经信任过」。
    func test_真文件_信任位是false也算没信任() throws {
        try write("""
        {"projects": {"\(workdir!)": {"hasTrustDialogAccepted": false}}}
        """, to: ".claude.json")

        let p = try XCTUnwrap(prompt(installed: [.claudeCode]))
        XCTAssertEqual(p.runners, [.claudeCode])
    }

    /// claude 信了、codex 没信 → **一个**提示，里面只说 codex 那条命令。
    /// （两家共用同一个提示是人类点名要的，不做两套。）
    func test_真文件_只有一家没信任时提示里只出现那一家() throws {
        try write("""
        {"projects": {"\(workdir!)": {"hasTrustDialogAccepted": true}}}
        """, to: ".claude.json")
        try write("""
        [projects."\(workdir!)"]
        trust_level = "untrusted"
        """, to: ".codex/config.toml")

        let p = try XCTUnwrap(prompt())
        XCTAssertEqual(p.runners, [.codex])
        XCTAssertEqual(p.commands, ["cd '\(workdir!)' && codex"])
        XCTAssertFalse(WorkdirTrustPrompt.body(p).contains("&& claude"),
                       "信过的那家不该出现在提示里")
    }

    /// 两家都信过 → 不打扰人。
    func test_真文件_两家都信任过则不提示() throws {
        try write("""
        {"projects": {"\(workdir!)": {"hasTrustDialogAccepted": true}}}
        """, to: ".claude.json")
        try write("""
        [projects."\(workdir!)"]
        trust_level = "trusted"
        """, to: ".codex/config.toml")

        XCTAssertNil(prompt())
    }

    /// 两个配置文件都不存在（全新机器）：读不到当没信任，而且**一个字都不写**。
    func test_真文件_配置不存在时不提示错也不写文件() throws {
        let p = try XCTUnwrap(prompt(), "读不到就当没信任，保守方向")
        XCTAssertEqual(p.runners, [.claudeCode, .codex])
        let fm = FileManager.default
        XCTAssertFalse(fm.fileExists(atPath: home.appendingPathComponent(".claude.json").path),
                       "检测是只读的，不许把文件建出来")
        XCTAssertFalse(fm.fileExists(atPath: home.appendingPathComponent(".codex").path))
    }

    /// 检测**全程只读**：跑完之后两份配置逐字节不动。
    func test_真文件_检测不改动任何配置() throws {
        try write("""
        {"projects": {"\(workdir!)": {"hasTrustDialogAccepted": false}}, "numStartups": 3}
        """, to: ".claude.json")
        try write("""
        [projects."\(workdir!)"]
        trust_level = "untrusted"
        """, to: ".codex/config.toml")
        let claudeBefore = try Data(contentsOf: home.appendingPathComponent(".claude.json"))
        let codexBefore = try Data(contentsOf: home.appendingPathComponent(".codex/config.toml"))

        _ = prompt()

        XCTAssertEqual(try Data(contentsOf: home.appendingPathComponent(".claude.json")),
                       claudeBefore)
        XCTAssertEqual(try Data(contentsOf: home.appendingPathComponent(".codex/config.toml")),
                       codexBefore)
    }

    /// 没装的 runner 不提示 —— 让人去跑一个他机器上没有的命令，比不提示更糟。
    func test_没装的runner不进提示() throws {
        let p = try XCTUnwrap(prompt(installed: [.codex]))
        XCTAssertEqual(p.runners, [.codex])
        XCTAssertNil(WorkdirTrustPrompt.prompt(workdir: workdir, home: home, installed: []))
    }

    // MARK: - 命令本身

    /// 路径里有空格 / 单引号也要能直接粘贴着跑。
    func test_命令对路径做了shell转义() {
        XCTAssertEqual(
            WorkdirTrustPrompt.command(for: .claudeCode, workdir: "/tmp/a b/it's here"),
            #"cd '/tmp/a b/it'\''s here' && claude"#)
    }

    /// 可执行名从 `LocalCodingAgentKind.binaryName` 算出来，不是这里拼死的字符串。
    func test_命令用的是runner自己的可执行名() {
        for kind in [LocalCodingAgentKind.claudeCode, .codex] {
            XCTAssertTrue(
                WorkdirTrustPrompt.command(for: kind, workdir: "/x").hasSuffix(
                    "&& " + kind.binaryName))
        }
    }

    // MARK: - 文案

    /// 提示只有一份：对话框正文和群聊那条**用的是同一批句子**。
    /// 写两遍的那天两份会各自漂，而漂了没有任何读数会报警。
    func test_对话框和群聊用同一批句子() throws {
        try write(#"{"projects": {}}"#, to: ".claude.json")
        let p = try XCTUnwrap(prompt())
        let body = WorkdirTrustPrompt.body(p)
        let chat = WorkdirTrustPrompt.chatMessage(p)
        for sentence in WorkdirTrustPrompt.sentences(p) {
            XCTAssertTrue(body.contains(sentence), "对话框缺了一句：\(sentence)")
            XCTAssertTrue(chat.contains(sentence), "群聊那条缺了一句：\(sentence)")
        }
    }

    /// **不许描述那个信任框长什么样。** 它的编号、选项顺序、默认高亮由上游说了算，
    /// 而且 11 天内整个换过一版 —— 写死任何一版形状，它就会在某天变成一句让人按错的假话。
    func test_文案不描述那个信任框的形状() throws {
        try write(#"{"projects": {}}"#, to: ".claude.json")
        let p = try XCTUnwrap(prompt())
        let text = WorkdirTrustPrompt.body(p) + "\n" + WorkdirTrustPrompt.chatMessage(p)
        for forbidden in ["选第", "第 1 项", "第 2 项", "默认高亮", "方向键", "按 2", "选项 2",
                         "Yes, I trust", "No, exit"] {
            XCTAssertFalse(text.contains(forbidden), "文案写死了会变的事实：\(forbidden)")
        }
    }
}
