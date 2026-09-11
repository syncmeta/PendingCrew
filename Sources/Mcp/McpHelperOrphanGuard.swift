import Darwin
import Foundation

/// helper 进程的自保：**爹没了就自己退。**
///
/// ## 为什么需要它（2026-09-11 实测，人类 Todo #111 第二轮）
///
/// `--mcp-serve` helper 唯一的退出条件是 `McpHelperMain` 那个读循环读到输入结束。
/// 正常情况下这够用：起它的那个 MCP 客户端（claude / codex）一死，管道的写端跟着
/// 关闭，helper 读到 EOF、循环退出、进程结束。
///
/// **但那个 EOF 是一条借来的前提** —— 它成立的条件不是「父进程死了」，而是
/// 「**再没有任何进程攥着那根管子的写端**」。这两件事平时同时发生，所以看起来
/// 像同一件事；只要有一个进程（父进程随手起的、继承了 fd 的任意子孙）还攥着
/// 写端，EOF 就永远不来 —— **父进程死了，helper 照样活着，被过继给 launchd，
/// 而且再也没有任何东西会来收它。**
///
/// 实测复现（真二进制、临时数据目录）：父进程 SIGKILL 之后 helper 存活、`ppid` 变成 1。
///
/// 这件事的代价不只是内存：每个 helper 是 **app 二进制自己再跑一遍**，而且它会去
/// 抢共享账本的 flock、在跨进程目录监听上占位。
///
/// ## 为什么修在 helper 自己身上
///
/// **app 全程拿不到 helper 的进程号** —— 它是 claude / codex 按 MCP 配置起的，不是
/// 我们起的（`LocalSessionLaunch.prepareLocalCommsConfig` / `codexMcpServers` 只写
/// 配置）。`SessionOrphanReaper` 那套双重核对记的是 **agent 进程**的 pid，覆盖不到
/// helper。所以 app 侧没有别的抓手，只能让 helper 自己守自己。
enum McpHelperOrphanGuard {

    /// 看门狗的默认节拍。取 30s：孤儿多活半分钟无所谓，而轮询本身要足够便宜 ——
    /// 这台机器上常年十几个 helper，每个都在转这个循环。
    static let defaultInterval: TimeInterval = 30

    enum Verdict: Equatable {
        case keepRunning
        /// 起我的那个进程已经不在了。
        case orphaned
    }

    /// 纯判定。**「被过继」就是「原来那个父进程死了」的唯一可观测形式** ——
    /// pid 不复用给父进程这件事在这里不需要担心：我们比的是「还是不是同一个爹」，
    /// 而不是「那个 pid 现在是谁」。爹换了人，无论换成谁，我都已经是孤儿。
    ///
    /// `currentParent <= 1` 单独列一条，是为了覆盖「一起来就已经是孤儿」
    /// （父进程在我 `getppid()` 之前就死了，初值本身就是 1）——
    /// 那种情况下 `current == initial`，只比「变没变」会漏掉它。
    static func verdict(initialParent: pid_t, currentParent: pid_t) -> Verdict {
        if currentParent <= 1 { return .orphaned }
        return currentParent == initialParent ? .keepRunning : .orphaned
    }

    /// 纯循环（注入取 ppid / 睡 / 判孤儿之后干什么）。**它不起线程、不碰真进程**，
    /// 所以每一条分支都测得到。
    ///
    /// `sleep` 返回 false = 别再睡了，循环收尾走人（测试用它保证不会转成死循环）。
    static func watch(
        initialParent: pid_t,
        interval: TimeInterval,
        currentParent: () -> pid_t,
        sleep: (TimeInterval) -> Bool,
        onOrphaned: () -> Void
    ) {
        while true {
            if verdict(initialParent: initialParent, currentParent: currentParent()) == .orphaned {
                onOrphaned()
                return
            }
            if !sleep(interval) { return }
        }
    }

    /// 真正装上去：起一个**普通 `Thread`**，轮询到孤儿就退出进程。
    ///
    /// ⚠️ **故意不用 async / 不碰 `@MainActor`。** helper 的主线程整个卡在
    /// `readLine` 里，主线程上的 hop 会永远排不上队 —— 同族的坑在这个进程里
    /// 咬过一次（helper 里的 `@MainActor` hop 挂死、fire-and-forget 静默不跑）。
    ///
    /// 退出前先等一个在途请求做完（`gate`）：共享账本的写是「flock + 临时文件
    /// rename」，半路死掉不会写坏文件，**但也没必要把一次已经开始的读-改-写扔掉**。
    /// 让「不会留下半截事务」成为我们检查过的事实，而不是靠别处的原子性顺带保证。
    static func install(gate: McpHelperInFlightGate,
                        interval: TimeInterval = McpHelperOrphanGuard.defaultInterval) {
        let initialParent = getppid()
        let thread = Thread {
            watch(initialParent: initialParent,
                  interval: interval,
                  currentParent: { getppid() },
                  sleep: { Foundation.Thread.sleep(forTimeInterval: $0); return true },
                  onOrphaned: {
                      gate.waitUntilIdle()
                      exit(0)
                  })
        }
        thread.name = "crew-helper-orphan-guard"
        thread.stackSize = 64 * 1024
        thread.start()
    }
}

/// 「此刻有没有一个请求正在处理」。看门狗退出进程之前拿它等一下。
///
/// 就是一把锁 —— 单独立个类型是为了让调用点读起来是「在途请求」而不是「某把锁」，
/// 免得将来有人顺手拿它去锁别的东西。
final class McpHelperInFlightGate: @unchecked Sendable {
    private let lock = NSLock()

    init() {}

    func withRequestInFlight<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }

    /// 等到没有请求在途。等到了就返回 —— 不保证之后不会再有新的（调用方接着就
    /// `exit`，而新请求只可能来自那个已经死掉的父进程）。
    func waitUntilIdle() {
        lock.lock()
        lock.unlock()
    }
}

/// serve 模式的读循环 + **它自己的看门狗**。
///
/// ## 为什么这个循环值得单独成一个类型
///
/// 从前它是 `runIfHelper` 里的四行 `while`。抽出来的唯一理由是：
/// **「看门狗有没有真的被装上」必须能被一条会红的测试盖住。** 只测
/// `McpHelperOrphanGuard` 那几个组件的话，把下面 `installGuard(gate)` 那一行删掉，
/// 那些测试照样全绿 —— 组件是对的，只是没人调它，而这正是变异测试要抓的形状。
///
/// 循环本身的行为一个字没变：读一行、交给 handler、有输出就写出去、读到结束就退。
struct McpHelperServeLoop {
    /// 读下一行。返回 nil = 输入结束（正常收工那条路）。
    var read: () -> String? = { readLine(strippingNewline: true) }
    var write: (String) -> Void = { print($0); fflush(stdout) }
    var handle: (String) -> String?
    /// 装看门狗。默认装真的；测试换成自己那只，好观察「装没装」。
    var installGuard: (McpHelperInFlightGate) -> Void = { McpHelperOrphanGuard.install(gate: $0) }

    func run() {
        let gate = McpHelperInFlightGate()
        installGuard(gate)
        while let line = read() {
            if let out = gate.withRequestInFlight({ handle(line) }) { write(out) }
        }
    }
}
