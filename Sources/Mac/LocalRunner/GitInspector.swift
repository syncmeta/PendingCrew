#if os(macOS)
import Foundation

/// 只读 git 查询(branch 列表 / repo root 判定 / 当前 branch),给
/// CreateSessionSheet 的 "新 worktree" 选项配 branch picker 用。
///
/// **故意做小**:
/// - 不写任何 mutating 命令(`git worktree add` 真到 T3.6 LocalRunner 真起
///   worktree 时才接;这里只查)。
/// - 不依赖第三方 git 库 —— 直接 spawn `/usr/bin/env git`,argv 形式传参,
///   不走 shell(spec v2 §8.4 同款 anti-injection 原则)。
/// - 全异步 + 静默错误:工作目录不是 repo / 没装 git 都返回 nil 或空 array,
///   不抛错;调用方 UI 上做"未检测到 git 仓库"的灰显即可。
enum GitInspector {
    /// 在 `directory` 跑 `git rev-parse --show-toplevel`;非 repo / git 未装
    /// 都返回 nil。
    static func repoRoot(at directory: URL) async -> URL? {
        guard let output = await run(["rev-parse", "--show-toplevel"], cwd: directory) else {
            return nil
        }
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        return URL(fileURLWithPath: trimmed)
    }

    /// 当前 HEAD branch 名;detached HEAD / 非 repo 返回 nil。
    static func currentBranch(at directory: URL) async -> String? {
        guard let output = await run(["rev-parse", "--abbrev-ref", "HEAD"], cwd: directory) else {
            return nil
        }
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty || trimmed == "HEAD" { return nil }
        return trimmed
    }

    /// 列本地 + 远程 branch (去掉 origin/HEAD alias)。给 picker 用,排序、
    /// 去重后返回。非 repo 返回 []。
    ///
    /// 实现:`git for-each-ref --format=%(refname:short) refs/heads refs/remotes`,
    /// 比 `git branch -a` 更稳(后者会带 `*` 当前 marker / `remotes/` 前缀)。
    static func listBranches(at directory: URL) async -> [String] {
        guard let output = await run([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ], cwd: directory) else {
            return []
        }
        let lines = output.split(separator: "\n").map { String($0) }
        var seen = Set<String>()
        var result: [String] = []
        for raw in lines {
            let name = raw.trimmingCharacters(in: .whitespaces)
            if name.isEmpty { continue }
            // 跳过 `origin/HEAD -> origin/main` 之类 symbolic alias —— 用
            // for-each-ref 不会出 alias 但 conservative 一下挡了 "->" 行。
            if name.contains(" -> ") { continue }
            // 标准化:`origin/main` 在 picker 里也允许选(用户可能从远端起新分支)。
            if seen.insert(name).inserted {
                result.append(name)
            }
        }
        // 本地分支优先(无 `/`),其次远程(`origin/...`),组内字母序。
        return result.sorted { lhs, rhs in
            let lLocal = !lhs.contains("/")
            let rLocal = !rhs.contains("/")
            if lLocal != rLocal { return lLocal }
            return lhs.lowercased() < rhs.lowercased()
        }
    }

    // MARK: - process

    /// 跑 `git <args>` 在 `cwd` 下,捕获 stdout。失败(非 0 退出 / spawn 失败 /
    /// git 未装)返回 nil,不抛。
    ///
    /// 不走 shell:Process.executableURL = `/usr/bin/env`,arguments = `["git"] + args`,
    /// 让 env 帮忙找 `git`(homebrew / Xcode CLT 装哪都行)。
    private static func run(_ args: [String], cwd: URL) async -> String? {
        await withCheckedContinuation { (cont: CheckedContinuation<String?, Never>) in
            DispatchQueue.global(qos: .userInitiated).async {
                let process = Process()
                process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
                process.arguments = ["git"] + args
                process.currentDirectoryURL = cwd
                let stdout = Pipe()
                let stderr = Pipe()
                process.standardOutput = stdout
                process.standardError = stderr
                do {
                    try process.run()
                } catch {
                    cont.resume(returning: nil)
                    return
                }
                process.waitUntilExit()
                guard process.terminationStatus == 0 else {
                    cont.resume(returning: nil)
                    return
                }
                let data = stdout.fileHandleForReading.readDataToEndOfFile()
                cont.resume(returning: String(data: data, encoding: .utf8))
            }
        }
    }
}
#endif
