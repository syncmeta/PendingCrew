#if os(macOS)
import Foundation

/// 解析 session 的工作目录（spec §8 / §11 worktree 隔离）。
/// `isolation=on` 且 crew 目录是 git repo → 在当前分支上开一个独立 worktree（新分支）；
/// `isolation=off` → 用 crew 共享目录。机长明确选择隔离后，任何创建失败都向上抛出；
/// 绝不静默退回共享目录，否则实际执行环境会违背机长的 session 启动决策。
enum SessionWorkspace {
    enum WorkspaceError: LocalizedError {
        case detachedHead(URL)

        var errorDescription: String? {
            switch self {
            case .detachedHead(let directory):
                return "无法为 session 新建 worktree：\(directory.path) 当前是 detached HEAD。"
            }
        }
    }

    static func resolve(crewDirectory: URL, isolation: Bool, hint: String) throws -> URL {
        guard isolation else { return crewDirectory }
        guard try GitWorktreeService.isGitRepository(at: crewDirectory) else {
            throw GitWorktreeService.GitError.notAGitRepo(crewDirectory)
        }
        guard let baseBranch = try GitWorktreeService.currentBranch(at: crewDirectory) else {
            throw WorkspaceError.detachedHead(crewDirectory)
        }
        let suffix = UUID().uuidString.lowercased().prefix(6)
        let newBranch = "pendingcrew/session-\(suffix)"
        return try GitWorktreeService.addWorktree(
            sourceRepository: crewDirectory,
            branchName: baseBranch,
            newBranchName: newBranch,
            worktreeNameHint: hint.isEmpty ? "session" : hint
        )
    }
}
#endif
