import XCTest

/// helper 的自保：**爹没了就自己退**（人类 Todo #111 第二轮）。
///
/// 这一组守的是一个**此刻还没有发作、但机制已经实测复现过**的泄漏：helper 唯一的
/// 退出条件是读到输入结束，而那个 EOF 的真实条件是「再没有进程攥着管子写端」——
/// 不是「父进程死了」。两者平时同时发生，所以看起来像同一件事。
///
/// ⚠️ **光证明这把尺子会红不够** —— 一个永远说「是孤儿」的判据，在真孤儿身上也会
/// 红，读数长得一模一样。所以下面「不是孤儿时必须说不是」那几条和「是孤儿时必须
/// 说是」那几条一样重要。
final class McpHelperOrphanGuardTests: XCTestCase {

    // MARK: - 纯判定：会红的那半

    func test_被过继给launchd时判作孤儿() {
        XCTAssertEqual(McpHelperOrphanGuard.verdict(initialParent: 500, currentParent: 1),
                       .orphaned)
    }

    func test_爹换了人就判作孤儿_不管换成谁() {
        XCTAssertEqual(McpHelperOrphanGuard.verdict(initialParent: 500, currentParent: 777),
                       .orphaned)
    }

    /// 起来时父进程就已经死了 —— 初值本身就是 1，`current == initial`。
    /// **只比「变没变」会漏掉这一种**，所以 `<= 1` 要单独成一条。
    func test_一起来就已经是孤儿也判得出来() {
        XCTAssertEqual(McpHelperOrphanGuard.verdict(initialParent: 1, currentParent: 1),
                       .orphaned)
    }

    func test_ppid为0这种脏值也当孤儿() {
        XCTAssertEqual(McpHelperOrphanGuard.verdict(initialParent: 500, currentParent: 0),
                       .orphaned)
    }

    // MARK: - 纯判定：会绿的那半（没有这几条，上面那几条不算数）

    func test_爹还是原来那个就继续跑() {
        XCTAssertEqual(McpHelperOrphanGuard.verdict(initialParent: 500, currentParent: 500),
                       .keepRunning)
    }

    func test_爹是别的进程号但没变过_照样继续跑() {
        XCTAssertEqual(McpHelperOrphanGuard.verdict(initialParent: 99_999, currentParent: 99_999),
                       .keepRunning)
    }

    // MARK: - 轮询循环

    func test_父进程一直活着时循环不会误报孤儿() {
        var fired = 0
        var slept = 0
        McpHelperOrphanGuard.watch(
            initialParent: 500, interval: 7,
            currentParent: { 500 },
            sleep: { interval in
                XCTAssertEqual(interval, 7)
                slept += 1
                return slept < 5          // 睡够 5 轮就收工，免得死循环
            },
            onOrphaned: { fired += 1 })
        XCTAssertEqual(fired, 0, "父进程一直在，一次都不该报孤儿")
        XCTAssertEqual(slept, 5)
    }

    func test_父进程中途没了时报一次孤儿并收尾() {
        var fired = 0
        var tick = 0
        McpHelperOrphanGuard.watch(
            initialParent: 500, interval: 1,
            currentParent: { tick += 1; return tick < 3 ? 500 : 1 },
            sleep: { _ in true },         // 永远愿意接着睡：循环必须靠判孤儿自己停下
            onOrphaned: { fired += 1 })
        XCTAssertEqual(fired, 1, "报一次就该走人，不该接着转")
        XCTAssertEqual(tick, 3)
    }

    // MARK: - 在途请求闸

    func test_在途请求没做完时不放看门狗过去() {
        let gate = McpHelperInFlightGate()
        let entered = DispatchSemaphore(value: 0)
        let finished = DispatchSemaphore(value: 0)
        var requestDone = false

        Thread.detachNewThread {
            gate.withRequestInFlight {
                entered.signal()
                Thread.sleep(forTimeInterval: 0.3)
                requestDone = true
            }
            finished.signal()
        }

        XCTAssertEqual(entered.wait(timeout: .now() + 5), .success)
        gate.waitUntilIdle()
        XCTAssertTrue(requestDone, "waitUntilIdle 在请求还没做完时就放行了")
        XCTAssertEqual(finished.wait(timeout: .now() + 5), .success)
    }

    func test_没有在途请求时不阻塞() {
        let gate = McpHelperInFlightGate()
        gate.waitUntilIdle()          // 卡住就是超时失败，不需要断言
        XCTAssertEqual(gate.withRequestInFlight { 42 }, 42)
    }

    // MARK: - 接线：这一条才是变异测试的靶子

    /// **这条是整组里唯一覆盖「看门狗有没有真的被装上」的。**
    ///
    /// 只测上面那些组件的话，把 `McpHelperServeLoop.run()` 里装看门狗的那一行删掉，
    /// 上面每一条都照样绿 —— 那正是要抓的形状：组件是对的，只是没人调它。
    ///
    /// 场景就是实测复现的那个：**输入永远不结束**（别的进程攥着管子写端），
    /// 于是读循环自己永远走不到头，只能靠看门狗把进程收掉。
    func test_输入永远不结束时靠看门狗收场() {
        let releaseRead = DispatchSemaphore(value: 0)
        let exited = XCTestExpectation(description: "看门狗判出孤儿并收尾")

        let loop = McpHelperServeLoop(
            // 攥着写端的那个进程还在 → EOF 永远不来。
            // 等不到就 2 秒后放行，让没装看门狗时是**红**而不是挂住。
            read: { _ = releaseRead.wait(timeout: .now() + 2); return nil },
            write: { _ in XCTFail("这一轮不该有输出") },
            handle: { _ in nil },
            installGuard: { gate in
                Thread.detachNewThread {
                    McpHelperOrphanGuard.watch(
                        initialParent: 500, interval: 0,
                        currentParent: { 1 },           // 已被过继 = 爹没了
                        sleep: { _ in true },
                        onOrphaned: {
                            gate.waitUntilIdle()        // 真进程在这里 exit(0)
                            exited.fulfill()
                            releaseRead.signal()
                        })
                }
            })

        loop.run()
        wait(for: [exited], timeout: 5)
    }

    /// 反面：输入正常结束时，读循环照旧把每一行喂给 handler、把输出写出去。
    /// **加了看门狗不许改变正常那条路**。
    func test_正常读循环不受影响() {
        var pending: [String] = ["a", "b"]
        var written: [String] = []
        let loop = McpHelperServeLoop(
            read: { pending.isEmpty ? nil : pending.removeFirst() },
            write: { written.append($0) },
            handle: { $0 == "a" ? "A" : nil },
            installGuard: { _ in })
        loop.run()
        XCTAssertEqual(written, ["A"], "只有 a 有输出，b 返回 nil 不该写出空行")
        XCTAssertTrue(pending.isEmpty, "输入应该被读干净")
    }
}
