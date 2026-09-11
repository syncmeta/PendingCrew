import XCTest

/// 装了新版，正在跑的 session 却拿不到新工具（人类当面点名那件事）。
///
/// 现场：`--mcp-serve` helper 是个长命进程，跟着 session 活好几天；**MCP 的工具表
/// 在它启动那一刻就定死了**。装新版只换磁盘上的文件，活着的进程还是旧的。于是
/// 机长去用当天新加的那档 Todo 状态，被拒成「status 只能是 pending / in_progress /
/// completed」—— 这句话对两件完全不同的事说得一模一样：
///   ① 这个能力这一版根本不存在；
///   ② 这一版有，只是你这个进程太老。
/// 一个 agent 读到它只会得出「这个功能不存在」然后绕路走。
///
/// 这一族钉的就是「它得说得出自己分不出这两件事」。
///
/// **红绿两面都要证**：只证明尺子会红不够 —— 一个永远喊「版本不符」的检测器在
/// 正常情况下也会喊，看起来一样对。所以下半截全是「已知同版 / 看不出来」的场景，
/// 它必须闭嘴。
final class HelperBuildStalenessTests: XCTestCase {

    private func tempDir(_ tag: String) -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("helper-build-\(tag)-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    /// 造一个真的 app bundle 骨架（`X.app/Contents/MacOS/X` + `Contents/Info.plist`），
    /// 返回可执行文件路径。**用真文件走真的 stat / plist 解析**，不造替身：
    /// 替身会带着我此刻的世界模型，跟实现一起错还互相背书。
    @discardableResult
    private func makeBundle(_ root: URL, version: String, build: String,
                            bytes: Int, commit: String? = nil) -> URL {
        let macos = root.appendingPathComponent("PendingCrew.app/Contents/MacOS")
        try? FileManager.default.createDirectory(at: macos, withIntermediateDirectories: true)
        let exe = macos.appendingPathComponent("PendingCrew")
        try? Data(repeating: 0x41, count: bytes).write(to: exe)
        var plist: [String: Any] = [
            "CFBundleShortVersionString": version,
            "CFBundleVersion": build,
        ]
        // 真实的 app bundle 带构建戳（`stamp-build-info.sh` 写的 BuildStampCommit）——
        // 两次构建版本号可以一模一样，commit 不会。
        if let commit { plist["BuildStampCommit"] = commit }
        let data = try! PropertyListSerialization.data(
            fromPropertyList: plist, format: .xml, options: 0)
        try? data.write(to: root.appendingPathComponent("PendingCrew.app/Contents/Info.plist"))
        return exe
    }

    private func server(dir: URL, watch: HelperBuildWatch?) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: "sess-1",
                  quotaDirectory: dir,
                  todos: LocalTodoStore(directory: dir),
                  plans: CockpitPlanStore(directory: dir),
                  buildWatch: watch)
    }

    /// 一句注定被拒的调用（note 空 → `ERROR: note 不能为空`）。
    private func refusedCall(_ s: McpServer) -> String {
        toolText(s.handleLine(
            #"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"continue_work","arguments":{"note":"  "}}}"#) ?? "")
    }

    /// 一句会成功的调用（空白板 → 正常回执，不是拒绝）。
    private func okCall(_ s: McpServer) -> String {
        toolText(s.handleLine(
            #"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"read_whiteboard","arguments":{}}}"#) ?? "")
    }

    private func toolText(_ raw: String) -> String {
        guard let data = raw.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let result = obj["result"] as? [String: Any],
              let content = result["content"] as? [[String: Any]],
              let text = content.first?["text"] as? String
        else { return raw }
        return text
    }

    // MARK: - 会红的那半：进程比磁盘旧，必须当着 agent 的面说出来

    /// 整份换掉（装新版就是这个形状：文件被替换，版本号也变了）。
    func test_磁盘上已经是新版_拒绝回执要说清是这个进程太老() {
        let dir = tempDir("stale")
        let exe = makeBundle(dir, version: "0.1.30", build: "250910", bytes: 128,
                             commit: "aaaaaaa1111")
        let old = HelperBuildStamp.read(executable: exe)
        XCTAssertNotNil(old, "前提没立住：连自己这份都读不出来")

        // 人类装了新版：同一个路径上换成另一份二进制 + 另一份 Info.plist。
        makeBundle(dir, version: "0.1.32", build: "250911", bytes: 4096,
                   commit: "bbbbbbb2222")

        let watch = HelperBuildWatch(running: old,
                                     probe: { HelperBuildStamp.read(executable: exe) })
        let text = refusedCall(server(dir: dir, watch: watch))

        XCTAssertTrue(text.hasPrefix("ERROR:"), "原来那句拒绝不能被吃掉：\(text)")
        XCTAssertTrue(text.contains("0.1.30"), "得说清我这个进程跑的是哪一版：\(text)")
        XCTAssertTrue(text.contains("0.1.32"), "得说清磁盘上现在是哪一版：\(text)")
        XCTAssertTrue(text.contains("工具表"), "得说清为什么换不掉：\(text)")
        XCTAssertTrue(text.contains("重开这个 session"), "得给出唯一那条出路：\(text)")
        // 构建戳那一列也要在 —— 版本号一样、commit 不一样是开发机上的常态形状。
        XCTAssertTrue(text.contains("aaaaaaa"), "得带上我这份的构建戳：\(text)")
        XCTAssertTrue(text.contains("bbbbbbb"), "得带上磁盘那份的构建戳：\(text)")
    }

    /// 原地覆写：inode 不变，只有 mtime / 大小动了。版本号可能一个字都没改
    /// （开发机上同一版重新构建就是这个形状）——照样要报，因为工具表确实对不上了。
    func test_版本号没变但二进制被覆写_照样要报() {
        let dir = tempDir("overwrite")
        let exe = makeBundle(dir, version: "0.1.32", build: "250911", bytes: 128)
        let old = HelperBuildStamp.read(executable: exe)
        makeBundle(dir, version: "0.1.32", build: "250911", bytes: 900)

        let watch = HelperBuildWatch(running: old,
                                     probe: { HelperBuildStamp.read(executable: exe) })
        XCTAssertNotNil(watch.staleNotice(), "同版号不同文件也是「工具表对不上」")
    }

    // MARK: - 会绿的那半：已知同版 / 看不出来时必须闭嘴

    /// 最要紧的一条绿：**什么都没发生的时候它不能叫**。
    /// 一个永远叫的检测器和一个真检测器，在坏场景里长得一模一样。
    func test_磁盘上就是我这一份_一个字都不加() {
        let dir = tempDir("same")
        let exe = makeBundle(dir, version: "0.1.32", build: "250911", bytes: 128)
        let watch = HelperBuildWatch(running: HelperBuildStamp.read(executable: exe),
                                     probe: { HelperBuildStamp.read(executable: exe) })

        let with = refusedCall(server(dir: dir, watch: watch))
        let without = refusedCall(server(dir: tempDir("same-base"), watch: nil))
        XCTAssertEqual(with, without, "同版时回执必须跟没这个检测时逐字一样")
    }

    /// app 被挪走 / 删掉 → 读不出磁盘那份。拿不准就闭嘴，别把「问不出来」报成「旧了」。
    func test_磁盘上那份读不出来_闭嘴() {
        let dir = tempDir("gone")
        let exe = makeBundle(dir, version: "0.1.32", build: "250911", bytes: 128)
        let old = HelperBuildStamp.read(executable: exe)
        try? FileManager.default.removeItem(at: dir.appendingPathComponent("PendingCrew.app"))

        let watch = HelperBuildWatch(running: old,
                                     probe: { HelperBuildStamp.read(executable: exe) })
        XCTAssertNil(watch.staleNotice())
    }

    /// 自己那份没记下来（`Bundle.main.executableURL` 为 nil 之类）→ 同样闭嘴。
    func test_自己那份没记下来_闭嘴() {
        let dir = tempDir("norunning")
        let exe = makeBundle(dir, version: "0.1.32", build: "250911", bytes: 128)
        let watch = HelperBuildWatch(running: nil,
                                     probe: { HelperBuildStamp.read(executable: exe) })
        XCTAssertNil(watch.staleNotice())
    }

    /// 成功回执不挂。装了新版没重开 session 是常态，每条回执都带一句就成了背景噪音，
    /// 而一直亮着的提示等于没有提示。
    func test_成功回执不挂这句话() {
        let dir = tempDir("ok")
        let exe = makeBundle(dir, version: "0.1.30", build: "250910", bytes: 128)
        let old = HelperBuildStamp.read(executable: exe)
        makeBundle(dir, version: "0.1.32", build: "250911", bytes: 4096)

        let watch = HelperBuildWatch(running: old,
                                     probe: { HelperBuildStamp.read(executable: exe) })
        let text = okCall(server(dir: dir, watch: watch))
        XCTAssertFalse(text.contains("工具表"), "成功回执被污染了：\(text)")
        XCTAssertFalse(text.contains("0.1.32"), "成功回执被污染了：\(text)")
    }

    /// 压根没注入这条通道（app 进程、绝大多数单测）→ 行为与从前逐字一致。
    func test_没有这条通道时回执一个字不变() {
        let dir = tempDir("nowatch")
        let text = refusedCall(server(dir: dir, watch: nil))
        XCTAssertEqual(text, "ERROR: note 不能为空；写清下一轮第一件事。")
    }
}
