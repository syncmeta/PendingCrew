import XCTest
import Darwin

/// 界面 / `list_sessions` 上直接看得出「这个成员的 helper 跑在哪一版、是不是旧的」
/// （机长计划 #88 第二件）。
///
/// 现场：装新版只换磁盘上的文件，活着的 `--mcp-serve` helper 还是旧二进制。今天找出
/// 「谁还跑在旧的上」要人手工比 inode。**判据不能看路径** —— `ps` 里的 argv 写着
/// `/Applications/...`，实际在执行的可能是 Sparkle 挪进 Caches 的旧包。
///
/// 这一族分三截：
///   ① 纯判定（三态，读不出来 ≠ 一样）；
///   ② **真进程**：把一份真的 PendingCrew 可执行文件拷到临时目录、以 `--mcp-serve`
///      起起来，再照 Sparkle 的形状把包挪走、原路径换上新的 —— 必须判成「旧」；
///   ③ 接线：点名那一列、界面那枚标、编排者每拍写快照那一行，少一处就红。
final class HelperBuildPerMemberTests: XCTestCase {

    private func tempDir(_ tag: String) -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("helper-member-\(tag)-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func stamp(version: String? = "0.1.32 (20709.1) · bbbbbbb",
                       inode: UInt64? = 7, size: Int64? = 100,
                       modified: Date? = Date(timeIntervalSince1970: 1_789_000_000.25)) -> HelperBuildStamp {
        HelperBuildStamp(versionText: version, modified: modified, size: size, inode: inode)
    }

    // MARK: - ① 纯判定

    func test_同一份文件_判当前() {
        XCTAssertEqual(HelperBuildVerdict.judge(running: stamp(), onDisk: stamp()), .current)
    }

    /// Sparkle 挪包：跑的那份被挪走了，原路径上是另一个文件 → inode 不同。
    func test_inode不同_判旧() {
        XCTAssertEqual(HelperBuildVerdict.judge(running: stamp(inode: 7), onDisk: stamp(inode: 8)), .stale)
    }

    /// 原地覆写：inode 不变，大小 / mtime 变了。
    func test_同inode但大小或mtime变了_判旧() {
        XCTAssertEqual(HelperBuildVerdict.judge(running: stamp(size: 100), onDisk: stamp(size: 101)), .stale)
        XCTAssertEqual(HelperBuildVerdict.judge(
            running: stamp(modified: Date(timeIntervalSince1970: 1_789_000_000)),
            onDisk: stamp(modified: Date(timeIntervalSince1970: 1_789_000_060))), .stale)
    }

    /// **正在跑的那份的版本串读不出来**（它的包已经被挪走删掉了）不能让判定变成「旧」
    /// 或「新」—— 身份三样对得上就是同一个文件；版本串只是给人读的。
    func test_跑的那份版本串读不出来但文件身份一致_仍判当前() {
        XCTAssertEqual(HelperBuildVerdict.judge(running: stamp(version: nil), onDisk: stamp()), .current)
    }

    /// 读不出来 ≠ 一样。任一侧缺、或身份三样里缺一样，都只能是「判不了」。
    func test_任一侧或任一身份字段读不出来_判不了_绝不判当前() {
        XCTAssertEqual(HelperBuildVerdict.judge(running: nil, onDisk: stamp()), .unknown)
        XCTAssertEqual(HelperBuildVerdict.judge(running: stamp(), onDisk: nil), .unknown)
        XCTAssertEqual(HelperBuildVerdict.judge(running: stamp(inode: nil), onDisk: stamp()), .unknown)
        XCTAssertEqual(HelperBuildVerdict.judge(running: stamp(), onDisk: stamp(size: nil)), .unknown)
        XCTAssertEqual(HelperBuildVerdict.judge(running: stamp(modified: nil), onDisk: stamp()), .unknown)
    }

    /// 已有的拒绝话术（`HelperBuildWatch.notice`）走的是同一把尺子 —— 不许有第二种口径。
    func test_拒绝话术与点名同一把尺子() {
        let launched = Date(timeIntervalSince1970: 1_789_000_000)
        // 版本串读不出来但文件是同一个：点名判当前，拒绝话术也必须闭嘴。
        XCTAssertNil(HelperBuildWatch.notice(running: stamp(version: nil), onDisk: stamp(),
                                             launchedAt: launched))
        XCTAssertNotNil(HelperBuildWatch.notice(running: stamp(inode: 1), onDisk: stamp(inode: 2),
                                                launchedAt: launched))
    }

    // MARK: - ① 聚合：一个 session 名下的 helper 进程 → 一格

    private func record(pid: Int32 = 100, crew: String? = "c", session: String? = "s",
                        running: HelperBuildStamp?) -> HelperProcessRecord {
        HelperProcessRecord(pid: pid, crewId: crew, sessionId: session,
                            launchPath: "/Applications/PendingCrew.app/Contents/MacOS/PendingCrew",
                            running: running)
    }

    func test_没找到helper进程_判不了_并说明原因() {
        let r = HelperProcessForensics.report(crewId: "c", sessionId: "s", helpers: [],
                                              onDisk: { _ in self.stamp() })
        XCTAssertEqual(r.verdict, .unknown)
        XCTAssertEqual(r.helperCount, 0)
        XCTAssertNotNil(r.reason)
    }

    func test_别的session和别的crew的helper不算() {
        let helpers = [record(session: "other", running: stamp(inode: 1)),
                       record(crew: "c2", running: stamp(inode: 1))]
        let r = HelperProcessForensics.report(crewId: "c", sessionId: "s", helpers: helpers,
                                              onDisk: { _ in self.stamp(inode: 2) })
        XCTAssertEqual(r.verdict, .unknown, "别人的旧 helper 被算到我头上了")
        XCTAssertEqual(r.helperCount, 0)
    }

    func test_一个旧一个新_整格判旧_因为旧的那个在答它的工具调用() {
        let helpers = [record(pid: 1, running: stamp(version: "0.1.30 (1) · aaaaaaa", inode: 1)),
                       record(pid: 2, running: stamp())]
        let r = HelperProcessForensics.report(crewId: "c", sessionId: "s", helpers: helpers,
                                              onDisk: { _ in self.stamp() })
        XCTAssertEqual(r.verdict, .stale)
        XCTAssertEqual(r.helperCount, 2)
        XCTAssertEqual(r.runningVersion, "0.1.30 (1) · aaaaaaa")
        XCTAssertEqual(r.diskVersion, "0.1.32 (20709.1) · bbbbbbb")
    }

    func test_一个新一个判不了_整格判不了_不许说成新的() {
        let helpers = [record(pid: 1, running: nil), record(pid: 2, running: stamp())]
        let r = HelperProcessForensics.report(crewId: "c", sessionId: "s", helpers: helpers,
                                              onDisk: { _ in self.stamp() })
        XCTAssertEqual(r.verdict, .unknown)
    }

    func test_全是当前_判当前_带上版本() {
        let r = HelperProcessForensics.report(crewId: "c", sessionId: "s",
                                              helpers: [record(running: stamp())],
                                              onDisk: { _ in self.stamp() })
        XCTAssertEqual(r.verdict, .current)
        XCTAssertEqual(r.runningVersion, "0.1.32 (20709.1) · bbbbbbb")
    }

    /// 旧 helper 起的时候不带 `--crew`（不会，但 argv 解不出来时是 nil）→ 只按 session 认。
    func test_argv里没解出crew时按session认() {
        let r = HelperProcessForensics.report(crewId: "c", sessionId: "s",
                                              helpers: [record(crew: nil, running: stamp(inode: 1))],
                                              onDisk: { _ in self.stamp(inode: 2) })
        XCTAssertEqual(r.verdict, .stale)
    }

    // MARK: - ① KERN_PROCARGS2 解析

    /// 手搭一份 `KERN_PROCARGS2` 的字节：argc(int32) + exec path + 填充的 \0 + argv + env。
    func test_procargs2解析_exec路径与argv() {
        var bytes: [UInt8] = []
        withUnsafeBytes(of: Int32(3).littleEndian) { bytes += $0 }
        bytes += Array("/Apps/X.app/Contents/MacOS/PendingCrew".utf8) + [0, 0, 0, 0]
        for a in ["/Apps/X.app/Contents/MacOS/PendingCrew", "--mcp-serve", "--session"] {
            bytes += Array(a.utf8) + [0]
        }
        bytes += Array("HOME=/Users/x".utf8) + [0]
        let parsed = HelperProcessForensics.parseProcArgs(bytes)
        XCTAssertEqual(parsed?.execPath, "/Apps/X.app/Contents/MacOS/PendingCrew")
        XCTAssertEqual(parsed?.argv, ["/Apps/X.app/Contents/MacOS/PendingCrew", "--mcp-serve", "--session"],
                       "argc 之后的环境变量不许混进 argv")
    }

    func test_procargs2解析_截断的字节不崩_返回nil() {
        XCTAssertNil(HelperProcessForensics.parseProcArgs([1, 0]))
        var bytes: [UInt8] = []
        withUnsafeBytes(of: Int32(5).littleEndian) { bytes += $0 }
        bytes += Array("/x".utf8) + [0, 0] + Array("a".utf8) + [0]
        XCTAssertNil(HelperProcessForensics.parseProcArgs(bytes), "argc 说 5 个、实际 1 个 → 解不出来")
    }

    // MARK: - ② 真进程：真的 PendingCrew 二进制、真的 --mcp-serve、真的挪包

    /// 找一份真的 PendingCrew 可执行文件。优先同一次构建的产物（test bundle 旁边那个 app），
    /// 其次本机装的。
    private func realExecutable() throws -> URL {
        let products = Bundle(for: Self.self).bundleURL.deletingLastPathComponent()
        let candidates = [
            products.appendingPathComponent("PendingCrew.app/Contents/MacOS/PendingCrew"),
            URL(fileURLWithPath: "/Applications/PendingCrew.app/Contents/MacOS/PendingCrew"),
        ]
        guard let hit = candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0.path) }) else {
            throw XCTSkip("本机找不到任何一份 PendingCrew 可执行文件（\(candidates.map(\.path))），真进程这条跑不了")
        }
        return hit
    }

    /// 在 `root` 下造一个 `PendingCrew.app`：可执行文件是真二进制的拷贝（`extraBytes`
    /// 追加在尾巴上，好让「新版」是另一份内容），Info.plist 写指定版本。
    @discardableResult
    private func makeRealBundle(at root: URL, from exe: URL, version: String, commit: String,
                                extraBytes: Int = 0) throws -> URL {
        let macos = root.appendingPathComponent("PendingCrew.app/Contents/MacOS")
        try FileManager.default.createDirectory(at: macos, withIntermediateDirectories: true)
        let dst = macos.appendingPathComponent("PendingCrew")
        try FileManager.default.copyItem(at: exe, to: dst)
        if extraBytes > 0 {
            let h = try FileHandle(forWritingTo: dst)
            try h.seekToEnd()
            try h.write(contentsOf: Data(repeating: 0, count: extraBytes))
            try h.close()
        }
        let plist: [String: Any] = ["CFBundleShortVersionString": version,
                                    "CFBundleVersion": "1", "BuildStampCommit": commit]
        try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0)
            .write(to: root.appendingPathComponent("PendingCrew.app/Contents/Info.plist"))
        return dst
    }

    private func waitForHelper(session: String, timeout: TimeInterval = 15) -> HelperProcessRecord? {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if let hit = HelperProcessForensics.scanHelpers().first(where: { $0.sessionId == session }) {
                return hit
            }
            Thread.sleep(forTimeInterval: 0.2)
        }
        return nil
    }

    func test_真进程_Sparkle把包挪走换上新版_argv路径没变_必须判旧() throws {
        let exe = try realExecutable()
        let root = tempDir("sparkle")
        let apps = root.appendingPathComponent("Applications")
        let data = root.appendingPathComponent("data")
        try FileManager.default.createDirectory(at: data, withIntermediateDirectories: true)
        let installed = try makeRealBundle(at: apps, from: exe, version: "0.1.30", commit: "aaaaaaa1111")

        let session = "helper-member-real-\(UUID().uuidString)"
        let proc = Process()
        proc.executableURL = installed
        proc.arguments = ["--mcp-serve", "--crew", "c-real", "--dir", data.path,
                          "--session", session, "--agent", "claude"]
        var env = ProcessInfo.processInfo.environment
        env["PENDINGCREW_DATA_DIR"] = data.path   // 绝不碰真数据目录
        proc.environment = env
        let stdin = Pipe()
        proc.standardInput = stdin            // 攥着写端 → helper 停在 readLine 上不退
        proc.standardOutput = FileHandle.nullDevice
        proc.standardError = FileHandle.nullDevice
        try proc.run()
        defer {
            proc.terminate()
            try? stdin.fileHandleForWriting.close()
            proc.waitUntilExit()
        }

        guard let before = waitForHelper(session: session) else {
            return XCTFail("起了真 helper（pid \(proc.processIdentifier)），扫描器却一直找不到它 —— 它是否活着：\(proc.isRunning)")
        }
        XCTAssertEqual(before.pid, proc.processIdentifier)
        XCTAssertEqual(before.crewId, "c-real")
        XCTAssertEqual(before.launchPath, installed.path)

        // 绿面：还没换包 → 必须判当前，而且报得出 0.1.30。
        let fresh = HelperProcessForensics.report(crewId: "c-real", sessionId: session,
                                                  helpers: [before],
                                                  onDisk: { HelperBuildStamp.read(executable: URL(fileURLWithPath: $0)) })
        XCTAssertEqual(fresh.verdict, .current, "没换包就判旧 = 永远在喊的检测器：\(fresh)")
        XCTAssertTrue(fresh.runningVersion?.contains("0.1.30") == true, "\(fresh)")

        // Sparkle 的形状：旧包整个挪进 Caches，原路径放上新包。
        let caches = root.appendingPathComponent("Caches/org.sparkle-project.Sparkle/Installation/x")
        try FileManager.default.createDirectory(at: caches, withIntermediateDirectories: true)
        try FileManager.default.moveItem(at: apps.appendingPathComponent("PendingCrew.app"),
                                         to: caches.appendingPathComponent("PendingCrew.app"))
        try makeRealBundle(at: apps, from: exe, version: "0.1.32", commit: "bbbbbbb2222", extraBytes: 64)

        guard let after = waitForHelper(session: session) else {
            return XCTFail("换包之后找不到这个 helper 了 —— 它是否活着：\(proc.isRunning)")
        }
        // 陷阱本身：argv 里的路径跟换包前一字不差，看路径永远判「一样」。
        XCTAssertEqual(after.launchPath, installed.path)

        let stale = HelperProcessForensics.report(crewId: "c-real", sessionId: session,
                                                  helpers: [after],
                                                  onDisk: { HelperBuildStamp.read(executable: URL(fileURLWithPath: $0)) })
        XCTAssertEqual(stale.verdict, .stale, "Sparkle 挪包没判出旧：\(stale)")
        XCTAssertTrue(stale.runningVersion?.contains("0.1.30") == true,
                      "跑的那份的版本得从挪走的那个包里读，而不是从原路径上的新包读：\(stale)")
        XCTAssertTrue(stale.diskVersion?.contains("0.1.32") == true, "\(stale)")

        // Sparkle 收尾会把挪走的旧包删掉：版本串读不出来了，但它**仍然是旧的**。
        try FileManager.default.removeItem(at: root.appendingPathComponent("Caches"))
        let gone = HelperProcessForensics.scanHelpers().first(where: { $0.sessionId == session })
        let afterDelete = HelperProcessForensics.report(crewId: "c-real", sessionId: session,
                                                        helpers: gone.map { [$0] } ?? [],
                                                        onDisk: { HelperBuildStamp.read(executable: URL(fileURLWithPath: $0)) })
        XCTAssertNotEqual(afterDelete.verdict, .current, "旧包删掉之后被判成新的了：\(afterDelete)")
        XCTAssertFalse(afterDelete.runningVersion?.contains("0.1.32") == true,
                       "跑的那份被说成了磁盘上那份的版本：\(afterDelete)")
    }

    // MARK: - ③ 点名那一列

    private func report(_ v: HelperBuildVerdict, running: String? = "0.1.30 (1) · aaaaaaa",
                        disk: String? = "0.1.32 (2) · bbbbbbb", reason: String? = nil) -> HelperBuildReport {
        HelperBuildReport(verdict: v, runningVersion: running, diskVersion: disk, reason: reason, helperCount: 1)
    }

    func test_点名列_旧的要说清两个版本和出路() {
        let col = HelperBuildReport.rosterColumn(.report(report(.stale)))
        XCTAssertTrue(col.contains("0.1.30"), col)
        XCTAssertTrue(col.contains("0.1.32"), col)
        XCTAssertTrue(col.contains("旧"), col)
        XCTAssertTrue(col.contains("重开"), col)
    }

    func test_点名列_判不了四种来源都不许说成新的或一致() {
        let cases: [HelperBuildLookup] = [
            .report(report(.unknown, running: nil, disk: nil, reason: "没找到它的 helper 进程")),
            .writerTooOld, .notInSnapshot, .unreadable("EPERM"),
        ]
        for c in cases {
            let col = HelperBuildReport.rosterColumn(c)
            XCTAssertTrue(col.contains("判不了"), "\(c) → \(col)")
            XCTAssertFalse(col.contains("一致"), "\(c) → \(col)")
            XCTAssertFalse(col.contains("旧"), "判不了不是旧：\(c) → \(col)")
        }
        XCTAssertTrue(HelperBuildReport.rosterColumn(.writerTooOld).contains("后台"),
                      "写快照的进程太老这件事要说出来（它和「没找到 helper」要人做的事不一样）")
    }

    func test_点名渲染每个活着的成员都带版本列_已退出的不带() {
        var snap = CrewSessionsSnapshot()
        snap.updatedAt = "2026-09-13T12:00:00Z"
        var stale = CrewSessionsSnapshot.Entry(sessionId: "w-1", name: "阿甲", role: "worker",
                                               brief: "", state: "idle")
        stale.helperBuild = report(.stale)
        let old = CrewSessionsSnapshot.Entry(sessionId: "w-2", name: "阿乙", role: "worker",
                                             brief: "", state: "working")   // 写快照的是旧后台
        let exited = CrewSessionsSnapshot.Entry(sessionId: "w-3", name: "阿丙", role: "worker",
                                                brief: "", state: "exited")
        snap.crews["c"] = [stale, old, exited]
        let out = snap.renderRoster(crewId: "c") { _ in .noOutput }
        let lines = out.split(separator: "\n").filter { $0.hasPrefix("- ") }
        XCTAssertEqual(lines.count, 3)
        XCTAssertTrue(lines[0].contains("0.1.30") && lines[0].contains("0.1.32"), String(lines[0]))
        XCTAssertTrue(lines[1].contains("判不了"), String(lines[1]))
        XCTAssertFalse(lines[2].contains("helper"), "已退出的成员没有 helper，挂一格只是噪音：\(lines[2])")
    }

    /// 端到端：快照文件 → 机长的 `list_sessions` 回执。
    func test_list_sessions回执里真的有这一列() throws {
        let dir = tempDir("list")
        var snap = CrewSessionsSnapshot()
        snap.updatedAt = "2026-09-13T12:00:00Z"
        var e = CrewSessionsSnapshot.Entry(sessionId: "w-1", name: "阿甲", role: "worker",
                                           brief: "改登录", state: "idle")
        e.helperBuild = report(.stale)
        snap.crews["local-org"] = [e]
        try JSONEncoder().encode(snap).write(to: dir.appendingPathComponent(CrewSessionsSnapshot.fileName))
        let server = McpServer(store: LocalWhiteboardStore(directory: dir),
                               approvals: LocalApprovalStore(directory: dir),
                               control: LocalCrewControlStore(directory: dir),
                               crewId: "local-org", sessionId: "cap", isCaptain: true,
                               quotaDirectory: dir,
                               agentSessions: LocalAgentSessionStore(directory: dir),
                               outputProbe: SessionOutputProbe(claudeProjectsDirectory: dir,
                                                               codexSessionsDirectory: dir))
        let out = server.handleLine(
            #"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_sessions","arguments":{}}}"#) ?? ""
        XCTAssertTrue(out.contains("0.1.30") && out.contains("0.1.32"), out)
    }

    /// 旧快照（没有 helperBuild 这个键）照样解得开 —— 新 helper 读旧后台写的文件。
    func test_没有这一格的旧快照照样解得开() throws {
        let json = #"{"crews":{"c":[{"sessionId":"s","name":"n","role":"worker","brief":"","state":"idle"}]},"updatedAt":"x"}"#
        let snap = try JSONDecoder().decode(CrewSessionsSnapshot.self, from: Data(json.utf8))
        XCTAssertNil(snap.crews["c"]?.first?.helperBuild)
    }

    // MARK: - ③ 界面那枚标（判定在这里，不在 View 里）

    func test_界面标_旧的是警告色_判不了是问号_不是版本号() {
        let stale = HelperBuildBadge.make(.report(report(.stale)), isRunning: true)
        XCTAssertEqual(stale?.tone, .warning)
        XCTAssertTrue(stale?.help.contains("0.1.30") == true && stale?.help.contains("0.1.32") == true)

        let current = HelperBuildBadge.make(.report(report(.current, running: "0.1.32 (2) · bbbbbbb")),
                                            isRunning: true)
        XCTAssertEqual(current?.tone, .neutral)
        XCTAssertEqual(current?.text, "0.1.32", "当前那枚只露短版本号")

        for c: HelperBuildLookup in [.report(report(.unknown, running: nil, disk: nil, reason: "r")),
                                     .writerTooOld, .notInSnapshot, .unreadable("EPERM")] {
            let b = HelperBuildBadge.make(c, isRunning: true)
            XCTAssertEqual(b?.tone, .unknown, "\(c)")
            XCTAssertFalse(b?.text.contains("0.1") == true, "判不了的标上不许出现版本号：\(c) → \(String(describing: b))")
        }
    }

    func test_界面标_已退出不挂() {
        XCTAssertNil(HelperBuildBadge.make(.report(report(.stale)), isRunning: false))
    }

    // MARK: - ③ 界面读快照的查表

    func test_查表_文件不在_读不动_没这个人_没这一格_各是各的() throws {
        let missing = CrewSessionsSnapshot.helperBuildLookupTable(directory: tempDir("nofile"))
        XCTAssertEqual(missing.lookup(sessionId: "s"), .notInSnapshot)

        let broken = tempDir("broken")
        try Data("{not json".utf8).write(to: broken.appendingPathComponent(CrewSessionsSnapshot.fileName))
        if case .unreadable = CrewSessionsSnapshot.helperBuildLookupTable(directory: broken)
            .lookup(sessionId: "s") {} else { XCTFail("解不开的快照没报成读不出来") }

        let dir = tempDir("table")
        var snap = CrewSessionsSnapshot()
        var withCell = CrewSessionsSnapshot.Entry(sessionId: "a", name: "a", role: "worker", brief: "", state: "idle")
        withCell.helperBuild = report(.stale)
        snap.crews["c"] = [withCell,
                           .init(sessionId: "b", name: "b", role: "worker", brief: "", state: "idle")]
        try JSONEncoder().encode(snap).write(to: dir.appendingPathComponent(CrewSessionsSnapshot.fileName))
        let table = CrewSessionsSnapshot.helperBuildLookupTable(directory: dir)
        XCTAssertEqual(table.lookup(sessionId: "a"), .report(report(.stale)))
        XCTAssertEqual(table.lookup(sessionId: "b"), .writerTooOld)
        XCTAssertEqual(table.lookup(sessionId: "zzz"), .notInSnapshot)
    }

    // MARK: - ③ 接线（编排者那一行、界面那一行不进 test bundle，只能按源码盯）

    private func source(_ relative: String) throws -> String {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        return try String(contentsOf: root.appendingPathComponent(relative), encoding: .utf8)
    }

    func test_接线_编排者每拍取证并把结果写进快照() throws {
        let runner = try source("Sources/Mac/Services/CrewSessionRunner.swift")
        XCTAssertTrue(runner.contains("HelperProcessForensics.reports(for:"),
                      "快照那一拍没人取证 —— 这一格永远是空的，界面和点名永远「判不了」")
        XCTAssertTrue(runner.contains("helperBuild: helperBuilds["),
                      "取了证没写进快照条目")
    }

    func test_接线_成员列表挂了这枚标() throws {
        let view = try source("Sources/Mac/Views/CrewSessionWindowView.swift")
        XCTAssertTrue(view.contains("HelperBuildBadge.make("), "成员行上没有这枚标")
        XCTAssertTrue(view.contains("CrewSessionsSnapshot.helperBuildLookupTable("), "界面没有去读快照")
    }
}
