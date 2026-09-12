#if os(macOS)
import XCTest

/// **开场 brief 到底送没送到** —— 这一组是 P5a 那条硬前置 bug 的回归网。
///
/// 病历（2026-09-04 机长在真 daemon 与正常界面模式**两边都实测到**）：daemon 拉起
/// 一个 claude session，TUI 正常起来（banner 都画出来了），但派给它的 brief 一个字
/// 都没进输入框，`~/.claude/projects/<slug>/<id>.jsonl` 里零条对话。它就那么空着等，
/// 点名看上去是「空闲」。手动 nudge 一句之后一切正常。
///
/// 病根不是 daemon 独有 —— 两条路共用同一个 `AgentSessionCore`。旧判据是
/// 「收到**首批** PTY 字节 + `Task.sleep(100ms)`」然后 `send(prompt)`：首批字节只证明
/// 子进程**开始画**了，不证明**输入框已经画好**。claude 首屏那几次重绘（清屏 / 切
/// alternate screen / 重画输入框）会把这次写入吞掉，而且**吞掉之后没有任何人知道** ——
/// 不报错、不重试、状态照样 `.running`。这条 bug 的全部代价就是它不报错。
///
/// ## 这一组测的是「规矩」，不是「某个时延」
///
/// 四条各自钉一条规矩，全部在**无画面**的 `AgentSessionCore` 上跑真 PTY：
/// 1. 输入框还没画出来 → **一个字节都不许投**（旧代码在这里失手）。
/// 2. 输入框画好了 → 投正文、**回读权威缓冲区确认它真的进了输入行**、再单发回车。
/// 3. 反复投不进去 → 到上限 → **翻 health**，不许静默（旧代码在这里静默）。
/// 4. 首屏是一个需要人回答的对话框（信任提示这种）→ **不投**，翻 health 说明卡在哪。
///
/// ## 为什么用一个假 TUI 而不是真 claude
///
/// 真 claude 要花订阅额度、要联网、每次结果都不一样 —— 那三条里任何一条都足以让它
/// 不配当 CI 里的一条测试（同 `AgentTuiFixtureRecorder` 的理由）。所以这里用一个
/// **只画到判据成立为止**的假 TUI（真 PTY、真 fork、真终端缓冲区），把「什么时候
/// 才算就绪」这条规矩钉死；而**判据本身认不认得真 claude 的画面**，由
/// `ClaudeInputBoxFixtureTests` 拿录下来的真字节验（`Tests/Fixtures/tui-claude.bin`）。
/// 两边合起来才是完整的：一边验规矩，一边验尺子。
@MainActor
final class StartupPromptDeliveryTests: XCTestCase {

    /// 开场 brief 里的可辨识标记。故意不含 shell 元字符 —— 它要原样穿过 PTY、
    /// 穿过假 TUI 的 `read`，再原样出现在屏幕上。
    private let marker = "PENDINGCREW_BRIEF_0451"

    // MARK: - ① 输入框还没画出来时，一个字节都不许投

    /// 假 TUI 先只画 banner（**没有输入框**）并把这段时间到达的输入全吞掉 ——
    /// 这正是真 claude 首屏重绘在做的事；延时结束后才画输入框、才开始读。
    ///
    /// 旧代码在这里两条都红：banner 一到就投（屏幕上当场看得见 brief 的字），
    /// 而那笔投递被吞掉，`GOT:` 那行永远不出现。
    func testDoesNotDeliverBeforeTheInputBoxIsDrawn() async throws {
        let script = try makeFakeTui(mode: "normal", readyDelay: 3)
        let core = makeCore(script: script)
        defer { core.stop() }
        // **不看当前画面，看累积字节流**：假 TUI（和真 TUI 一样）每次重绘都清屏，
        // 早投的那一笔在画面上只停留一瞬就被擦掉了 —— 只看画面会漏判。回显进来的
        // 字节留在流里，不会被擦。
        let seen = ByteBox()
        core.onOutput = { seen.append($0) }

        // 输入框就绪之前的整个窗口里持续采样：一个字节都不许投出去。
        let deadline = Date().addingTimeInterval(2.5)
        while Date() < deadline {
            XCTAssertFalse(
                seen.text.contains(marker),
                """
                输入框还没画出来就把开场 brief 投出去了 —— 这一笔会被首屏重绘吞掉，\
                而且吞掉之后没有任何人知道。
                """)
            try? await Task.sleep(nanoseconds: 50_000_000)
        }

        // 就绪之后才该投，而且要真的落进去、被提交。
        let landed = await waitUntil(20) { core.screenText(maxLines: 60).contains("GOT:\(self.marker)") }
        XCTAssertTrue(
            landed,
            """
            输入框就绪之后开场 brief 仍然没送到。当前画面：
            \(core.screenText(maxLines: 60))
            """)
        XCTAssertNil(core.health, "正常送达的 session 不该翻 health")
    }

    // MARK: - ② 就绪后投递并校验成功

    /// 假 TUI 一上来就画好输入框。要验的是整条链路走完：正文进输入行 → 回车 →
    /// 输入行清空。`GOT:` 那行同时证明了「进了输入框」和「真的被提交了」。
    func testDeliversAndSubmitsOnceTheInputBoxIsReady() async throws {
        let script = try makeFakeTui(mode: "normal", readyDelay: 0)
        let core = makeCore(script: script)
        defer { core.stop() }

        let landed = await waitUntil(20) { core.screenText(maxLines: 60).contains("GOT:\(self.marker)") }
        XCTAssertTrue(
            landed,
            """
            输入框已经就绪，开场 brief 却没被送达并提交。当前画面：
            \(core.screenText(maxLines: 60))
            """)
        XCTAssertNil(core.health, "正常送达的 session 不该翻 health")
    }

    /// crew 的真实开场正文包含整块 world model 与白板，可能长到终端把 `❯` 提示符
    /// 顶出当前屏幕。正文已经落进 PTY 时不能因为找不到提示符就永远不发回车。
    func testSubmitsWhenLongBriefScrollsTheInputMarkerOffScreen() async throws {
        let script = try makeFakeTui(mode: "normal", readyDelay: 1)
        // 小于 PTY canonical input 上限，但在 40×8 的屏幕里足以把提示符顶走。
        let longPrompt = Array(repeating: marker, count: 30).joined(separator: " ")
        let core = makeCore(script: script, prompt: longPrompt)
        core.resize(cols: 40, rows: 8)
        defer { core.stop() }

        let landed = await waitUntil(20) {
            core.screenText(maxLines: 60).contains("GOT:\(self.marker)")
        }
        XCTAssertTrue(
            landed,
            """
            长开场正文把输入提示符顶出屏幕后没有被提交。当前画面：
            \(core.screenText(maxLines: 60))
            """)
        XCTAssertNil(core.health, "长正文正常送达后不该翻 health")
    }

    // MARK: - ③ 反复投不进去 → 到上限 → 翻 health，不许静默

    /// 假 TUI 画好了输入框，但**关掉回显、永不读取** —— 从我们这边看就是「投了，
    /// 输入行却一直是空的」。真 claude 首屏被吞掉时长的就是这个样子。
    ///
    /// 旧代码在这里静默：投一次、没进去、没人知道，`status` 照样 `.running`，
    /// 点名把它报成「空闲」，机长照常派活。
    func testRaisesHealthWhenTheBriefNeverLandsInTheInputRow() async throws {
        let script = try makeFakeTui(mode: "deaf", readyDelay: 0)
        let core = makeCore(script: script)
        defer { core.stop() }

        let raised = await waitUntil(30) { core.health != nil }
        XCTAssertTrue(
            raised,
            """
            开场 brief 反复没进输入行，却一个字都没报 —— 这正是这条 bug 的全部代价。\
            当前画面：
            \(core.screenText(maxLines: 60))
            """)
        XCTAssertEqual(core.health?.kind, .briefUndelivered)
        XCTAssertTrue(
            core.health?.detail.contains("开场") == true,
            "health 说明要让人一眼看出卡在哪：\(core.health?.detail ?? "nil")")
        XCTAssertNotEqual(
            CrewSessionStateDerivation.state(
                isRunning: true, health: core.health, isWorking: false),
            "idle",
            "点名不许再把它报成「空闲」")
    }

    // MARK: - ④ 首屏是需要人回答的对话框 → 不投，翻 health

    /// 未信任的新目录里，claude 首屏是「是否信任此文件夹」。旧代码那一笔回车被
    /// 对话框吃掉、选中 `No, exit`，session 秒退且零输出 —— 同一条竞态的第二种翻车
    /// 形状（机长实测过）。这里假 TUI 把那个对话框原样画出来并**开着回显**，所以
    /// 一旦有任何投递，屏幕上当场看得见。
    func testDoesNotDeliverIntoAModalDialogAndRaisesHealth() async throws {
        let script = try makeFakeTui(mode: "dialog", readyDelay: 0)
        let core = makeCore(script: script)
        defer { core.stop() }
        let seen = ByteBox()
        core.onOutput = { seen.append($0) }

        let raised = await waitUntil(30) { core.health != nil }
        XCTAssertTrue(raised, "首屏卡在等人回答的对话框上，必须报出来（现场留给 inspect_session）")
        XCTAssertEqual(core.health?.kind, .briefUndelivered)
        XCTAssertTrue(
            core.health?.detail.contains("需要人回答") == true,
            "health 说明要指明卡在等人回答上：\(core.health?.detail ?? "nil")")

        XCTAssertFalse(
            seen.text.contains(marker),
            """
            对话框在场时把 brief 当按键投了进去 —— 回车会被对话框吃掉、选中「No, exit」，\
            session 秒退且零输出。
            """)
        XCTAssertTrue(core.isProcessRunning, "只报不答：现场要留着给 inspect_session")
    }

    // MARK: - ⑤ 运行中的 steer 走同一台机器（#15）

    /// **人类 2026-09-12 报的那条**：在一个 session 的输入框里看见自己没敲过的文字，
    /// session 就那么卡着 ——「不是我敲的。怎么会因为没提交的字而卡住？」
    ///
    /// 病根在旧的 `send(_:)`：写完正文起一个 `Task` 睡 200ms 再补回车，而那个 Task
    /// 在 `status != .running` 时直接 `return`；claude 把整笔当成 paste 时回车也会被
    /// 吞掉。两种情况下正文都原样躺在输入框里，**而没有任何人回看一眼**。
    ///
    /// 这一条钉的是：steer 必须**被提交**，不是「被打进去」。假 TUI 只有真的读到一
    /// 整行才会回显 `GOT:` —— 字进了输入框但没提交时，它一个字都不会回。
    ///
    /// ⚠️ **这条的边界（实测，不是推论）**：把 `send` 换回旧的 fire-and-forget 实现
    /// 跑一趟，本组四条新测里三条变红，**这一条照样绿**。原因是假 TUI 不模拟 claude 的
    /// 「粘贴 vs 敲键」判定，旧实现那笔 `正文 + 200ms 后回车` 在它面前是能成的。
    /// 所以它**不是**挡住这条 bug 的那把尺子 —— 挡住的是下面三条（没落地要报、
    /// 两笔要排队、已退出要报）。留着它是为了防新实现自己走丢，不是为了防旧实现回来。
    func testSteerIsSubmittedNotJustTypedIntoTheBox() async throws {
        let script = try makeFakeTui(mode: "normal", readyDelay: 0)
        let core = makeCore(script: script, prompt: nil, withBrief: false)
        defer { core.stop() }

        let ready = await waitUntil(10) { ClaudeInputBox.inputRow(core.screenRows()) != nil }
        XCTAssertTrue(ready, "假 TUI 的输入框没画出来，后面测什么都不算数")
        core.send(marker)

        let submitted = await waitUntil(20) {
            core.screenText(maxLines: 60).contains("GOT:\(self.marker)")
        }
        XCTAssertTrue(
            submitted,
            """
            steer 没被提交 —— 正文多半正躺在输入框里等人手动按回车，而这正是人类看到的\
            「不是我敲的字」。当前画面：
            \(core.screenText(maxLines: 60))
            """)
        XCTAssertNil(core.health, "正常送达不该翻 health")
    }

    /// 送不进去时**不许静默**。旧 `send` 在这里一个字都不报：写完就走，没人回看。
    ///
    /// 这条同时钉住 kind 必须是 `.promptUndelivered` 而不是 `.briefUndelivered` ——
    /// 两者共用一台编排机，但 `announce` 是按 kind 去重的，合成一类的话第二条以后
    /// 全被第一条吃掉，又变回静默。
    func testRaisesHealthWhenASteerNeverLands() async throws {
        let script = try makeFakeTui(mode: "deaf", readyDelay: 0)
        let core = makeCore(script: script, prompt: nil, withBrief: false)
        defer { core.stop() }

        let ready = await waitUntil(10) { ClaudeInputBox.inputRow(core.screenRows()) != nil }
        XCTAssertTrue(ready, "deaf 模式也该先把输入框画出来")
        core.send(marker)

        let raised = await waitUntil(30) { core.health != nil }
        XCTAssertTrue(raised, "steer 反复没进输入行，却一个字都没报 —— 这正是这条 bug 的全部代价")
        XCTAssertEqual(core.health?.kind, .promptUndelivered)
        XCTAssertFalse(
            core.health?.detail.contains("开场") == true,
            "运行中的 steer 不该说成「开场任务」：\(core.health?.detail ?? "nil")")
    }

    /// 两笔同时发 → **排队，不叠**。两段正文一起往同一个输入框写会糊成一段谁也看不懂
    /// 的东西，而重投时那下 Ctrl-U 只清得掉一笔。
    func testConcurrentSteersAreQueuedAndBothArrive() async throws {
        let script = try makeFakeTui(mode: "normal", readyDelay: 0)
        let core = makeCore(script: script, prompt: nil, withBrief: false)
        defer { core.stop() }

        let ready = await waitUntil(10) { ClaudeInputBox.inputRow(core.screenRows()) != nil }
        XCTAssertTrue(ready)
        core.send("FIRST_\(marker)")
        core.send("SECOND_\(marker)")

        let first = await waitUntil(20) {
            core.screenText(maxLines: 60).contains("GOT:FIRST_\(self.marker)")
        }
        XCTAssertTrue(first, "第一笔没到：\(core.screenText(maxLines: 60))")
        let second = await waitUntil(20) {
            core.screenText(maxLines: 60).contains("GOT:SECOND_\(self.marker)")
        }
        XCTAssertTrue(
            second,
            """
            第二笔没到 —— 排队里的那笔被吞了。当前画面：
            \(core.screenText(maxLines: 60))
            """)
        XCTAssertNil(core.health)
    }

    /// session 已经不在跑了还往里发 —— 旧代码就是在这一步 `return` 的，静默。
    /// 现在它必须留下一条账：这条指令没有被任何人接收。
    func testSendingToAnExitedSessionIsReportedNotDropped() async throws {
        let script = try makeFakeTui(mode: "normal", readyDelay: 0)
        let core = makeCore(script: script, prompt: nil, withBrief: false)
        core.stop()
        let exited = await waitUntil(5) { core.status != .running }
        XCTAssertTrue(exited)

        core.send(marker)
        XCTAssertEqual(
            core.health?.kind, .promptUndelivered,
            "往一个已经退出的 session 发指令，必须报出来而不是悄悄丢掉")
        XCTAssertTrue(
            core.health?.detail.contains(marker) == true,
            "账里要能看出丢的是哪条：\(core.health?.detail ?? "nil")")
    }

    // MARK: - 器材

    /// `withBrief: false` = 压根不给开场正文，于是开场那台投递机不会起 ——
    /// 运行中 steer 的那几条要的正是一个「已经在跑、手上没活」的 core。
    private func makeCore(
        script: String, prompt: String? = nil, withBrief: Bool = true
    ) -> AgentSessionCore {
        AgentSessionCore(
            config: SessionConfig(
                kind: .claudeCode, initialPrompt: withBrief ? (prompt ?? marker) : nil),
            mode: .agent,
            executable: script,
            workdir: NSTemporaryDirectory(),
            env: ["TERM": "xterm-256color", "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"])
    }

    @discardableResult
    private func waitUntil(
        _ timeout: TimeInterval = 8, _ cond: @MainActor () -> Bool
    ) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if cond() { return true }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        return cond()
    }

    /// 假 claude TUI。**真 PTY、真 fork**，只是画面是手画的 —— 它只需要长到能让
    /// 「输入框就绪」这条判据成立（提示符行 `❯`，下面还压着框线与提示条，所以
    /// 判据必须真的从底往上找，不能只看最后一行）。
    ///
    /// - `normal`：`readyDelay` 秒内只有 banner，且这段时间到达的输入**全部吞掉**
    ///   （`cat /dev/tty` —— 后台任务的 stdin 会被 sh 改成 /dev/null，所以必须
    ///   显式打开控制终端）；延时结束才画输入框并开始逐行读。
    /// - `deaf`：一上来就画好输入框，但关掉回显、永不读取 —— 投进去的东西不会
    ///   出现在输入行上。
    /// - `dialog`：画「是否信任此文件夹」，回显开着，什么都不读。
    ///
    /// 模式**烧进脚本文件本身**，不走环境变量：core 是拿一份显式 env 起进程的，
    /// 测试进程 `setenv` 的东西根本传不过去（第一版就是这么写的，四条全绿得莫名其妙）。
    private func makeFakeTui(mode: String, readyDelay: Int) throws -> String {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("pendingcrew-fake-tui-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent("fake-claude-tui.sh")
        let script = Self.fakeTuiScript
            .replacingOccurrences(of: "@MODE@", with: mode)
            .replacingOccurrences(of: "@READY_DELAY@", with: String(readyDelay))
        try script.write(to: url, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o755], ofItemAtPath: url.path)
        addTeardownBlock { try? FileManager.default.removeItem(at: dir) }
        return url.path
    }

    /// `\342\224\200` = ─（框线），`\342\235\257` = ❯（提示符/选择光标），
    /// `\302\267` = ·。用八进制转义写，免得 shell 脚本文件本身带非 ASCII。
    private static let fakeTuiScript = """
    #!/bin/sh
    # 假 claude TUI（PendingCrew 测试器材）。见 StartupPromptDeliveryTests 的注释。
    FAKE_TUI_MODE='@MODE@'
    FAKE_TUI_READY_DELAY='@READY_DELAY@'
    LAST=''

    line() { printf '\\342\\224\\200\\342\\224\\200\\342\\224\\200\\342\\224\\200\\342\\224\\200\\342\\224\\200\\342\\224\\200\\342\\224\\200\\r\\n'; }

    banner() {
      printf '\\033[2J\\033[H'
      printf 'Claude Code v2.1.246\\r\\n'
      printf 'Opus 5 with high effort \\302\\267 Claude Max\\r\\n'
    }

    box() {
      printf '\\033[2J\\033[H'
      printf 'Claude Code v2.1.246\\r\\n'
      printf '\\r\\n'
      printf '%s\\r\\n' "$LAST"
      printf '\\r\\n'
      line
      printf '\\342\\235\\257 \\r\\n'
      line
      printf '  auto mode on (shift+tab to cycle) \\302\\267 esc to interrupt\\r\\n'
      # 光标回到输入行 —— 回显进来的字符必须落在 ❯ 后面，跟真 TUI 一样。
      printf '\\033[6;3H'
    }

    dialog() {
      printf '\\033[2J\\033[H'
      printf 'Accessing workspace:\\r\\n'
      printf '\\r\\n'
      printf '%s\\r\\n' "$PWD"
      printf '\\r\\n'
      printf 'Quick safety check: Is this a project you created or one you trust?\\r\\n'
      printf '\\r\\n'
      printf "Claude Code'll be able to read, edit, and execute files here.\\r\\n"
      printf '\\r\\n'
      printf 'Security guide\\r\\n'
      printf '\\r\\n'
      printf '\\342\\235\\257 1. Yes, I trust this folder\\r\\n'
      printf '  2. No, exit\\r\\n'
      printf '\\r\\n'
      printf 'Enter to confirm \\302\\267 Esc to cancel\\r\\n'
    }

    case "$FAKE_TUI_MODE" in
      dialog)
        dialog
        sleep 600
        ;;
      deaf)
        stty -echo 2>/dev/null
        box
        sleep 600
        ;;
      *)
        if [ "$FAKE_TUI_READY_DELAY" -gt 0 ]; then
          # 首屏重绘阶段：这段时间到达的输入全被吞掉，真 claude 丢 brief 就是这么丢的。
          cat /dev/tty > /dev/null &
          SWALLOW=$!
          banner
          sleep "$FAKE_TUI_READY_DELAY"
          kill "$SWALLOW" 2>/dev/null
          wait "$SWALLOW" 2>/dev/null
        fi
        box
        while IFS= read -r reply; do
          # 只回显首个词，避免长输入自己的回显再次把确认行顶出测试屏幕。
          LAST="GOT:${reply%% *}"
          box
        done
        ;;
    esac
    """
}

/// 线程安全的字节累加器（`onOutput` 从 PTY 队列回调）。
private final class ByteBox: @unchecked Sendable {
    private let lock = NSLock()
    private var bytes: [UInt8] = []
    func append(_ slice: ArraySlice<UInt8>) {
        lock.lock(); bytes.append(contentsOf: slice); lock.unlock()
    }
    var text: String {
        lock.lock(); defer { lock.unlock() }
        return String(decoding: bytes, as: UTF8.self)
    }
}
#endif
