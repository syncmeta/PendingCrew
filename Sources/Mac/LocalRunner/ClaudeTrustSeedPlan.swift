#if os(macOS)
import Foundation

/// 「给一个**新**工作目录补种 claude 的信任记录」的**规划层**（纯判定，不碰文件系统）。
///
/// ## 为什么要有它
///
/// `~/.claude.json` 的 `projects["<绝对路径>"].hasTrustDialogAccepted` 记着「这个目录
/// 信任过没有」。建 crew 时我们会**新造**一个目录（`~/CrewGround/<地名>`），那个路径
/// 从来没有人进去过 —— 于是它下面起的第一个 claude 停在
/// 「Quick safety check: Is this a project you created or one you trust?」上：
/// 进程起来了、不吐第一个字、也不报错，点名显示成**「空闲」**。
/// 2026-09-06 实测：全机 565 条 `projects` 里 21 条信任位不是 `true`，其中 8 条正是
/// `~/CrewGround/<地名>`。
///
/// **踩坑现场的形状是「条目在、值是 `false`」，不是「条目缺失」** —— 所以「已存在就
/// 跳过」这种写法会原样把人卡在信任框上。
///
/// ## 跟 `WorkdirMigrationPlan` 是什么关系
///
/// 那边是**迁移**（换工作目录）时把旧路径的信任/权限搬到新路径，早就在搬这个位、
/// 也早就写下了「目标已有条目但值是 false 也要补」。这里是**创建**这条路 ——
/// 同一件事的另一半，此前没做。所以判定复用那边的类型与尺子
/// （`ClaudeProjectSettings` / `isMeaningful` / `normalize` / 键名单），
/// **不另立第二套**。
///
/// 唯一故意的不同：**补种只写 `hasTrustDialogAccepted` 一个键**。迁移搬 8 个键是因为
/// 源目录真有那些值；创建这条路没有源，凭空写 `allowedTools` / `mcpServers`
/// 等于替人放行工具 —— 那是另一件事，绝不顺手做。
///
/// ## 授权这道闸不在这一层
///
/// 「什么动作算拿到了人的授权」是界面层的事（人类 Todo #3 还没拍）。这一层只认
/// `granted` / `notGranted`，由调用方喂进来 —— 拍完板改的是调用方，不是这里。
enum ClaudeTrustSeedPlan {

    /// 有没有拿到人的授权去写这个位。**不是我们自己判的**，调用方给。
    enum Authorization: Equatable {
        case granted
        case notGranted
    }

    struct Inputs {
        var workdir: String
        var authorization: Authorization
        /// `~/.claude.json` 里这个路径当前的条目快照（执行层读真文件填进来）。
        var existing: WorkdirMigrationPlan.ClaudeProjectSettings

        init(workdir: String, authorization: Authorization,
             existing: WorkdirMigrationPlan.ClaudeProjectSettings) {
            self.workdir = workdir
            self.authorization = authorization
            self.existing = existing
        }
    }

    /// 补种要写的键。**就这一个**，理由见类型注释。
    static let seededKeys: [String] = ["hasTrustDialogAccepted"]

    /// 没做的事 —— 每一条都要能对人说清楚。
    enum Skip: Equatable {
        case emptyWorkdir
        /// 这个目录已经信任过了，本来就不用做（跟授权无关）。
        case alreadyTrusted(path: String)
        /// 要补，但没拿到授权 —— 一个字都不写。
        case notAuthorized(path: String)
    }

    enum Decision: Equatable {
        case seed(path: String, keys: [String])
        case skip(Skip)

        /// 真要写的键（skip 时为空）。
        var keysToSeed: [String] {
            if case .seed(_, let keys) = self { return keys }
            return []
        }
    }

    /// **纯函数**：只看 `inputs`，不碰文件系统。
    ///
    /// 顺序有讲究：先看「本来就不用做」，再看授权。已经信任过的目录报
    /// 「没授权」是撒谎 —— 那种情况下我们本来也不会写任何东西。
    static func decide(_ inputs: Inputs) -> Decision {
        let path = WorkdirMigrationPlan.normalize(inputs.workdir)
        guard !path.isEmpty else { return .skip(.emptyWorkdir) }
        // `isMeaningful(false) == false` —— 所以「条目在、值是 false」落在这里的
        // else 支，跟「压根没条目」走同一条路。这正是踩坑现场要的行为。
        guard !isTrusted(inputs.existing) else { return .skip(.alreadyTrusted(path: path)) }
        guard inputs.authorization == .granted else { return .skip(.notAuthorized(path: path)) }
        return .seed(path: path, keys: seededKeys)
    }

    /// 这份快照算不算「已经信任过」。用迁移那边同一把尺子
    /// （`meaningfulKeys` 只收「有实质值」的键，`false` 进不来）。
    static func isTrusted(_ settings: WorkdirMigrationPlan.ClaudeProjectSettings) -> Bool {
        settings.meaningfulKeys.contains(seededKeys[0])
    }
}
#endif
