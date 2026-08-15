#if os(macOS)
import Foundation

/// workspace 仓库的目录布局读写层 —— 只管「文件在哪、怎么原子写」，不碰 git/网络。
///
/// **设计动机**（spec §3 目录骨架已由用户确认）：
/// - 骨架：`workspace.toml` + `projects/<id>.toml` + `crews/` + `env/` + `secrets/` +
///   `machines/<machine-id>.toml`。`crews/` / `env/` / `secrets/` 在这一层只建空目录占位
///   （放 `.gitkeep` 让 git 能追踪空目录），内容分别由后续 StateSyncService /
///   EnvSyncService / SecretsSyncService 负责，不在这层展开。
/// - **`project-id` 就是文件名去掉 `.toml`**：不额外在 `ProjectManifest` 里存一份 id
///   字段，避免「文件名 vs 内部字段」两处真相不一致的坑（改名只需 mv 文件）。
/// - **machine override 优先**：`effectiveLocalPath` 语义 = 有对应机器 override 用
///   override，没有（或该机器压根没注册过 `machines/<id>.toml`）就退回
///   `project.localPath`——退回路径必须覆盖「机器文件不存在」和「文件存在但那个
///   project-id 没被 override」两种情况，都视作「未命中」。
/// - **原子写**：先写临时文件（同目录下 `.tmp` 后缀，保证和目标同一 volume，
///   `replaceItem` 才能用 rename 语义）再 `FileManager.replaceItemAt` 替换，避免同步
///   引擎写到一半被打断/被另一个进程读到半份 TOML。父目录不存在时自动创建——调用方
///   （SyncEngine）不需要先手动 mkdir。
public struct WorkspaceRepoLayout {
    public enum LayoutError: Error, Equatable {
        /// `root` 不是一个已初始化的 workspace 仓库（`workspace.toml` 不存在）。
        case notAWorkspace(URL)
    }

    public let root: URL

    public init(root: URL) {
        self.root = root
    }

    // MARK: - 路径约定

    private var workspaceFile: URL { root.appendingPathComponent("workspace.toml") }
    private var projectsDir: URL { root.appendingPathComponent("projects") }
    private var crewsDir: URL { root.appendingPathComponent("crews") }
    private var envDir: URL { root.appendingPathComponent("env") }
    private var secretsDir: URL { root.appendingPathComponent("secrets") }
    private var machinesDir: URL { root.appendingPathComponent("machines") }

    private func projectFile(id: String) -> URL {
        projectsDir.appendingPathComponent("\(id).toml")
    }

    private func machineFile(id: String) -> URL {
        machinesDir.appendingPathComponent("\(id).toml")
    }

    // MARK: - 读

    public func loadWorkspace() throws -> WorkspaceManifest {
        guard FileManager.default.fileExists(atPath: workspaceFile.path) else {
            throw LayoutError.notAWorkspace(root)
        }
        let toml = try String(contentsOf: workspaceFile, encoding: .utf8)
        return try WorkspaceManifestCodec.decode(WorkspaceManifest.self, toml: toml)
    }

    public func loadProjects() throws -> [String: ProjectManifest] {
        guard FileManager.default.fileExists(atPath: projectsDir.path) else {
            return [:]
        }
        let files = try FileManager.default.contentsOfDirectory(
            at: projectsDir, includingPropertiesForKeys: nil)
        var result: [String: ProjectManifest] = [:]
        for file in files where file.pathExtension == "toml" {
            let id = file.deletingPathExtension().lastPathComponent
            let toml = try String(contentsOf: file, encoding: .utf8)
            result[id] = try WorkspaceManifestCodec.decode(ProjectManifest.self, toml: toml)
        }
        return result
    }

    public func loadMachine(id: String) throws -> MachineManifest? {
        let file = machineFile(id: id)
        guard FileManager.default.fileExists(atPath: file.path) else {
            return nil
        }
        let toml = try String(contentsOf: file, encoding: .utf8)
        return try WorkspaceManifestCodec.decode(MachineManifest.self, toml: toml)
    }

    /// machine override 优先，否则 `project.localPath`；两种情况都会退回默认值：
    /// 该机器没注册过（`machines/<id>.toml` 不存在），或注册过但没覆盖这个 project-id。
    public func effectiveLocalPath(
        projectId: String, project: ProjectManifest, machineId: String
    ) throws -> URL {
        let raw: String
        if let machine = try loadMachine(id: machineId),
           let override = machine.localPathOverrides[projectId] {
            raw = override
        } else {
            raw = project.localPath
        }
        let expanded = (raw as NSString).expandingTildeInPath
        return URL(fileURLWithPath: expanded)
    }

    // MARK: - 写（原子）

    public func writeProject(id: String, _ p: ProjectManifest) throws {
        try atomicWrite(WorkspaceManifestCodec.encode(p), to: projectFile(id: id))
    }

    public func writeMachine(id: String, _ m: MachineManifest) throws {
        try atomicWrite(WorkspaceManifestCodec.encode(m), to: machineFile(id: id))
    }

    public func writeWorkspace(_ w: WorkspaceManifest) throws {
        try atomicWrite(WorkspaceManifestCodec.encode(w), to: workspaceFile)
    }

    private func atomicWrite(_ content: String, to url: URL) throws {
        let dir = url.deletingLastPathComponent()
        if !FileManager.default.fileExists(atPath: dir.path) {
            try FileManager.default.createDirectory(
                at: dir, withIntermediateDirectories: true)
        }
        let tmpURL = dir.appendingPathComponent(url.lastPathComponent + ".tmp")
        try content.write(to: tmpURL, atomically: false, encoding: .utf8)
        _ = try FileManager.default.replaceItemAt(url, withItemAt: tmpURL)
    }

    // MARK: - 骨架

    /// 建目录骨架：`workspace.toml` + `projects/` + `crews/` + `env/` + `secrets/` +
    /// `machines/`。空目录放 `.gitkeep`（git 不追踪空目录，占位保证结构在 clone
    /// 后依然完整可见）。
    public static func scaffold(at root: URL, name: String) throws -> WorkspaceRepoLayout {
        let layout = WorkspaceRepoLayout(root: root)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)

        for dir in [layout.projectsDir, layout.crewsDir, layout.envDir,
                    layout.secretsDir, layout.machinesDir] {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let gitkeep = dir.appendingPathComponent(".gitkeep")
            if !FileManager.default.fileExists(atPath: gitkeep.path) {
                try Data().write(to: gitkeep)
            }
        }

        try layout.writeWorkspace(WorkspaceManifest(name: name, schemaVersion: 1))
        return layout
    }
}
#endif
