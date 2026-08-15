#if os(macOS)
import Foundation

/// 实时汇总本机 Claude Code / Codex 今日 token 用量。
///
/// 数据来源：
/// - Claude Code：扫 ~/.claude/projects/**/*.jsonl（当日改动的文件），
///   累加每条 assistant 消息的 usage.input_tokens + cache_creation_input_tokens
///   + output_tokens（跳过 cache_read 避免历史上下文重复计数）。
/// - Codex：sqlite3 ~/.codex/state_5.sqlite，SUM(tokens_used) WHERE date = today。
///
/// 调用 start() 后立即计算一次，之后每 60s 轮询刷新。
/// 无 ~/.claude 或 ~/.codex 目录时对应值保持 nil，footer 不渲染那行。
@MainActor
final class LocalAgentUsageMonitor: ObservableObject {
    @Published private(set) var claudeTodayTokens: Int? = nil
    @Published private(set) var codexTodayTokens: Int? = nil

    private var pollTask: Task<Void, Never>?

    func start() {
        guard pollTask == nil else { return }
        doRefresh()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(60))
                if Task.isCancelled { return }
                self?.doRefresh()
            }
        }
    }

    func refresh() { doRefresh() }

    private func doRefresh() {
        Task { [weak self] in
            async let cc = Self.computeClaudeTokensToday()
            async let cx = Self.computeCodexTokensToday()
            let (ccVal, cxVal) = await (cc, cx)
            await MainActor.run { [weak self] in
                self?.claudeTodayTokens = ccVal
                self?.codexTodayTokens = cxVal
            }
        }
    }

    // MARK: - Claude Code

    private static func computeClaudeTokensToday() async -> Int? {
        await Task.detached(priority: .utility) {
            let fm = FileManager.default
            let projectsDir = fm.homeDirectoryForCurrentUser.appendingPathComponent(".claude/projects")
            guard let projDirs = try? fm.contentsOfDirectory(
                at: projectsDir,
                includingPropertiesForKeys: [.isDirectoryKey],
                options: .skipsHiddenFiles
            ) else { return nil }

            let todayStart = Calendar.current.startOfDay(for: Date())
            var total = 0
            var anyFound = false

            for projDir in projDirs {
                let isDir = (try? projDir.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
                guard isDir else { continue }
                guard let files = try? fm.contentsOfDirectory(
                    at: projDir,
                    includingPropertiesForKeys: [.contentModificationDateKey],
                    options: .skipsHiddenFiles
                ) else { continue }
                for file in files where file.pathExtension == "jsonl" {
                    let modDate = (try? file.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast
                    guard modDate >= todayStart else { continue }
                    if let t = sumClaudeTokensInJSONL(url: file) {
                        total += t
                        anyFound = true
                    }
                }
            }
            return anyFound ? total : nil
        }.value
    }

    /// 扫一个 session JSONL，把 assistant 条目的 usage 累加。
    /// 计入：input_tokens + cache_creation_input_tokens + output_tokens
    /// 跳过：cache_read_input_tokens（避免长对话历史被重复计数）
    private nonisolated static func sumClaudeTokensInJSONL(url: URL) -> Int? {
        guard let content = try? String(contentsOf: url, encoding: .utf8) else { return nil }
        var total = 0
        var found = false
        for line in content.split(separator: "\n", omittingEmptySubsequences: true) {
            guard let data = line.data(using: .utf8),
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  json["type"] as? String == "assistant",
                  let msg = json["message"] as? [String: Any],
                  let usage = msg["usage"] as? [String: Any] else { continue }
            total += (usage["input_tokens"] as? Int ?? 0)
            total += (usage["cache_creation_input_tokens"] as? Int ?? 0)
            total += (usage["output_tokens"] as? Int ?? 0)
            found = true
        }
        return found ? total : nil
    }

    // MARK: - Codex

    private static func computeCodexTokensToday() async -> Int? {
        await Task.detached(priority: .utility) {
            let dbPath = FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent(".codex/state_5.sqlite").path
            guard FileManager.default.fileExists(atPath: dbPath) else { return nil }
            let proc = Process()
            proc.executableURL = URL(fileURLWithPath: "/usr/bin/sqlite3")
            proc.arguments = [
                dbPath,
                "SELECT COALESCE(SUM(tokens_used),0) FROM threads WHERE date(created_at,'unixepoch','localtime')=date('now','localtime');"
            ]
            let outPipe = Pipe()
            proc.standardOutput = outPipe
            proc.standardError = Pipe()
            guard (try? proc.run()) != nil else { return nil }
            proc.waitUntilExit()
            let raw = String(data: outPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? "0"
            return Int(raw)
        }.value
    }
}

// MARK: - Format helper

extension LocalAgentUsageMonitor {
    /// 把原始 token 数格式化为 "450k" / "2.3M" / "15M"。
    static func formatTokens(_ n: Int) -> String {
        if n >= 1_000_000 {
            let m = Double(n) / 1_000_000
            return m >= 10 ? "\(Int(m.rounded()))M" : String(format: "%.1fM", m)
        }
        if n >= 1_000 {
            let k = Double(n) / 1_000
            return k >= 10 ? "\(Int(k.rounded()))k" : String(format: "%.1fk", k)
        }
        return "\(n)"
    }
}

#endif
