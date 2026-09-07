import Foundation

/// 销号凭据（Todo #102 第四刀）。**只在把一条 Todo 翻成「完成」时要求**。
///
/// ## 为什么要有它
/// 2026-09-07 挖出一笔假账：群里记着「bot 中文搜不到已修好，合进 main（`2cde0da9`）」——
/// 那个 hash **在两个仓都不是合法 git 对象**，那段有 bug 的代码一字未变，账挂了 191 小时。
///
/// 它比「一件事没人回」坏得多：那种账**保守**（事情比账面好或一样，代价是有人白等），
/// 这种账**乐观** —— 账面比事情好，长得跟成功一模一样，**所以不会有第二个人回来看**。
///
/// 关键不在于「它没带凭据」，它带了。**关键是那个凭据从来没有被解析过。**
/// 所以这里的规矩是：**给了指针就当场解引用，解不出来就拒绝销号。**
/// 「先记下来，以后再核」正好就是这笔假账的形状。
///
/// ## ⚠️ 我们能验的只有指针，不是内容
/// `git cat-file -e` 只证明**那个对象存在**，**不证明它做了那件事**。所以回执里
/// 只说「凭据 `<sha>` 解析成功」，绝不说「已验证该修复」。谁把这两句当同一件事，
/// 这道闸就白装了 —— 它拦的是**凭空捏造的 hash**，不是**指错的 hash**。
enum TodoEvidence {
    /// 解引用一个 hash 的结果。由调用方注入（真实实现跑 `git cat-file -e`），
    /// 判定本身保持纯函数、可单测。
    enum Resolution: Equatable {
        /// 这个对象在仓库里存在。
        case resolved
        /// 仓库在、但没有这个对象 —— 就是那笔假账的形状。
        case notFound
        /// **验不了**（这儿不是 git 仓库 / git 跑不起来 / 超时）。带上说明。
        case unavailable(String)
    }

    /// 判定结果。每一种在回执里都要说成不同的话 —— 压成 Bool 就等于把「为什么
    /// 不算数」丢在这一层，调用方只会瞎重试。
    enum Verdict: Equatable {
        /// hash 解析成功。带上规范化后的 hash。
        case commitResolved(String)
        /// 没给 hash，给了文字凭据（一次核对、一个结论、一次真机验收）。
        case prose(String)
        /// 给了 hash，仓库里没有 —— **拒绝销号**。
        case commitNotFound(String)
        /// 给了 hash，但这里验不了 —— **同样拒绝**，让它改走文字凭据那条路。
        /// 「验不了就先记下」是明令禁止的那一种。
        case cannotVerify(commit: String, why: String)
        /// 给的 hash 形状就不对（不是 7–40 位十六进制）—— 不必去跑 git 就知道。
        case malformedCommit(String)
        /// 两条都没给。
        case missing
    }

    /// hash 的形状：7–40 位十六进制。短于 7 位在大仓里必然歧义，长于 40 位不是 sha1。
    /// 先过形状再去跑 git —— 省一次进程，而且「形状不对」和「仓库里没有」是两件事，
    /// 说成同一句话会让人去错的方向找。
    static func looksLikeCommit(_ raw: String) -> Bool {
        let s = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard (7...40).contains(s.count) else { return false }
        return s.allSatisfy { $0.isHexDigit && $0.isASCII }
    }

    /// 判据本身。**顺序是先验后动账** —— 调用方拿到非 `.commitResolved` / `.prose`
    /// 的结果就什么都不许写。
    ///
    /// hash 优先：两个都给了也以 hash 为准（文字那条留在回执里，不丢）。
    static func judge(commit: String?, prose: String?,
                      resolve: (String) -> Resolution) -> Verdict {
        let hash = (commit ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let text = (prose ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !hash.isEmpty {
            guard looksLikeCommit(hash) else { return .malformedCommit(hash) }
            switch resolve(hash) {
            case .resolved: return .commitResolved(hash)
            case .notFound: return .commitNotFound(hash)
            case .unavailable(let why): return .cannotVerify(commit: hash, why: why)
            }
        }
        if !text.isEmpty { return .prose(text) }
        return .missing
    }
}

/// 真的去仓库里解引用一个 hash。`git cat-file -e <sha>^{object}`。
///
/// 自包含 Foundation（要跟着 `McpServer` 编进 `--mcp-serve` helper）。
/// **超时是硬要求**：helper 的主线程从头到尾卡在 `readLine` 上，这里挂住就是整个
/// session 不动了 —— 而「不动」在这台机器上一向比「报错」难查。
///
/// ## `directory` 必须是那条 crew **登记在册**的工作目录
/// 不许拿 helper 的 cwd 往上找 git 仓库根。往上找会找到「一个」仓库，但不保证是
/// 「那个」—— worktree、`/private/tmp` 下的发包树、别的项目仓都可能在祖先链上。
/// 那条路的失败形态不是「找不到」，是**在错的仓库里解析成功**：于是一条假账带着
/// 一句「凭据解析成功」挂上去。**验不了会逼人换条路；假绿不会。**
/// 拿不到登记的 workdir 时，调用方必须报「我验不了」，**不许猜一个**。
struct GitObjectProbe {
    /// 那条 crew 登记在册的工作目录（`LocalCrewStore.workingDirectory(crewId:...)`）。
    let directory: String
    /// 等多久算超时（秒）。
    let timeout: TimeInterval

    /// 这个平台**能不能**跑 git 去解引用。
    ///
    /// iOS 上「验凭据」这件事本来就不成立：没有 git、没有工作副本、也不会有人在
    /// 手机上销号。所以那儿的正确形状**不是想办法让它跑起来**，是让它落进已经
    /// 存在的那一态 —— `.unavailable`「我验不了，问题在环境不在你给的东西」。
    /// **不新造第三条路，也不把整个类型 `#if` 藏掉**（藏掉就要连调用点一起包）。
    enum PlatformSupport: Equatable {
        case canRunGit
        /// 带上「为什么这个平台验不了」。
        case unsupported(String)
    }

    /// 当前平台的判定。**这是唯一一处平台分支** —— `resolve` 拿它当普通值用，
    /// 于是「验不了」这条路在 Mac 上也测得到（把 `support` 传成 `.unsupported`）。
    static var current: PlatformSupport {
        #if os(macOS)
        return .canRunGit
        #else
        return .unsupported("这个平台上跑不了 git（凭据解析只在 Mac 上成立）")
        #endif
    }

    /// 平台能力。默认取当前平台；测试注入另一半，好让 iOS 那条路在 Mac 上也可测。
    let support: PlatformSupport

    init(directory: String, timeout: TimeInterval = 5,
         support: PlatformSupport = GitObjectProbe.current) {
        self.directory = directory
        self.timeout = timeout
        self.support = support
    }

    func resolve(_ hash: String) -> TodoEvidence.Resolution {
        if case .unsupported(let why) = support { return .unavailable(why) }
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: directory, isDirectory: &isDir),
              isDir.boolValue else {
            return .unavailable("登记的工作目录 \(directory) 现在不在了，我没法在那儿解析 commit")
        }
        guard FileManager.default.fileExists(atPath: "/usr/bin/git") else {
            return .unavailable("这台机器上找不到 /usr/bin/git")
        }
        // 只包住真正 macOS-only 的那一段（`Process` 在 iOS 上不存在）。
        // 上面那条 `support` 判断已经让非 Mac 走不到这里；这个 `#if` 是给**编译器**
        // 看的，不是第二条业务分支 —— 两者少了任何一个都不行：只有 `#if` 的话
        // 「iOS 上会怎样」测不到，只有 `support` 的话 iOS 根本编不过。
        #if os(macOS)
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
        process.arguments = ["-C", directory, "cat-file", "-e", "\(hash)^{object}"]
        process.standardOutput = FileHandle.nullDevice
        let err = Pipe()
        process.standardError = err
        do {
            try process.run()
        } catch {
            return .unavailable("跑不起来 git：\(error.localizedDescription)")
        }
        let deadline = Date().addingTimeInterval(timeout)
        while process.isRunning, Date() < deadline {
            usleep(20_000)
        }
        if process.isRunning {
            process.terminate()
            return .unavailable("git 超过 \(Int(timeout)) 秒没返回，已中止")
        }
        // 退出码 0 = 对象存在。非 0 有两种：这儿不是仓库（stderr 会说 not a git
        // repository），或者仓库里真没这个对象。**两者必须分开** —— 前者是「我验不了」，
        // 后者是「你给的凭据不存在」，把它们说成一句话就等于把假账放行。
        let stderr = String(data: err.fileHandleForReading.readDataToEndOfFile(),
                            encoding: .utf8) ?? ""
        if process.terminationStatus == 0 { return .resolved }
        if stderr.contains("not a git repository") || stderr.contains("不是 git 仓库") {
            return .unavailable("\(directory) 不是 git 仓库，这里没法解析 commit")
        }
        return .notFound
        #else
        // 走不到（上面 `support` 已经挡住），但编译器要它。**说的话跟那条一致**，
        // 别在这里发明第二套措辞。
        return .unavailable("这个平台上跑不了 git（凭据解析只在 Mac 上成立）")
        #endif
    }
}
