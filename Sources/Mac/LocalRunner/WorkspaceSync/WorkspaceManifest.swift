#if os(macOS)
import Foundation
import TOMLKit

/// workspace 仓库 manifest 模型 + TOML 编解码。
///
/// **设计动机**（schema 见
/// `docs/superpowers/specs/2026-07-19-pendingcrew-workspace-sync-design.md` §3）：
/// - workspace 仓库用纯文本 TOML 存 manifest（`workspace.toml` / `projects/<id>.toml` /
///   `machines/<machine-id>.toml`），人可读可手改，冲突时 `git diff` 一眼看懂 —— 不用
///   JSON/YAML，TOML 是 Cargo/pyproject 生态验证过的「配置给人看」格式。
/// - 字段名 Swift 侧用 camelCase（Swift 惯例），落盘用 snake_case（TOML/spec 手写示例的
///   格式）—— 每个 struct 显式声明 `CodingKeys` 做桥接，不依赖 TOMLKit 自带的 key-转换策略。
/// - `LastSync.pushedAt` 存成 ISO8601 字符串（不是 TOML 原生 datetime 类型）：字符串在
///   diff / 日志里更直观，也避免把「Date 编解码」这个额外维度引进第一版 manifest 层。
///   （spec 里手写示例用了原生 datetime 字面量，这里按 plan Task 1 的接口定义收口成字符串。）
/// - `WorkspaceManifestCodec` 薄薄包一层 TOMLEncoder/TOMLDecoder —— 调用方（后续的
///   SyncEngine / UI）只认 Codable 类型和 TOML 字符串，TOMLKit 这个第三方依赖被隔离在这一个
///   文件里，换库或升级只用改这里。

/// `workspace.toml` —— workspace 仓库的根 manifest：工作空间名 + schema 版本。
public struct WorkspaceManifest: Codable, Equatable, Sendable {
    public var name: String
    public var schemaVersion: Int

    enum CodingKeys: String, CodingKey {
        case name
        case schemaVersion = "schema_version"
    }

    public init(name: String, schemaVersion: Int) {
        self.name = name
        self.schemaVersion = schemaVersion
    }
}

/// 同步引擎写入的回执（`projects/<id>.toml` 里的 `[last_sync]` 表）——人不手改。
public struct LastSync: Codable, Equatable, Sendable {
    public var machine: String
    public var branch: String
    public var head: String
    /// ISO8601 字符串，见文件头注释。
    public var pushedAt: String

    enum CodingKeys: String, CodingKey {
        case machine
        case branch
        case head
        case pushedAt = "pushed_at"
    }

    public init(machine: String, branch: String, head: String, pushedAt: String) {
        self.machine = machine
        self.branch = branch
        self.head = head
        self.pushedAt = pushedAt
    }
}

/// `projects/<project-id>.toml` —— 每个项目仓库一份声明（不含仓库内容本身，项目仓库
/// 不进 workspace 仓库）。
public struct ProjectManifest: Codable, Equatable, Sendable {
    public var name: String
    public var remote: String
    public var localPath: String
    public var defaultBranch: String
    /// 首次声明时还没同步过，允许为 nil；同步引擎跑过一次之后才落盘。
    public var lastSync: LastSync?

    enum CodingKeys: String, CodingKey {
        case name
        case remote
        case localPath = "local_path"
        case defaultBranch = "default_branch"
        case lastSync = "last_sync"
    }

    public init(
        name: String,
        remote: String,
        localPath: String,
        defaultBranch: String,
        lastSync: LastSync? = nil
    ) {
        self.name = name
        self.remote = remote
        self.localPath = localPath
        self.defaultBranch = defaultBranch
        self.lastSync = lastSync
    }
}

/// `machines/<machine-id>.toml` —— 每机覆盖（`local_path` 等）+ 设备注册信息。
public struct MachineManifest: Codable, Equatable, Sendable {
    public var displayName: String
    /// age 加密（§7 凭证层）用的公钥指纹；设备还没配凭证层时为 nil。
    public var agePublicKeyFingerprint: String?
    /// key = project-id，value = 该机的 local_path 覆盖值。
    public var localPathOverrides: [String: String]

    enum CodingKeys: String, CodingKey {
        case displayName = "display_name"
        case agePublicKeyFingerprint = "age_public_key_fingerprint"
        case localPathOverrides = "local_path_overrides"
    }

    public init(
        displayName: String,
        agePublicKeyFingerprint: String? = nil,
        localPathOverrides: [String: String] = [:]
    ) {
        self.displayName = displayName
        self.agePublicKeyFingerprint = agePublicKeyFingerprint
        self.localPathOverrides = localPathOverrides
    }
}

/// 薄薄一层包住 TOMLKit 的 `TOMLEncoder`/`TOMLDecoder`。
///
/// 隔离依赖：workspace-sync 后续的代码（SyncEngine、UI）只调用这两个静态方法，不直接
/// `import TOMLKit`——TOMLKit 只在这一个文件里出现，换库/升版本影响面收在这里。
public enum WorkspaceManifestCodec {
    public static func decode<T: Decodable>(_ type: T.Type, toml: String) throws -> T {
        try TOMLDecoder().decode(type, from: toml)
    }

    public static func encode<T: Encodable>(_ value: T) throws -> String {
        try TOMLEncoder().encode(value)
    }
}
#endif
