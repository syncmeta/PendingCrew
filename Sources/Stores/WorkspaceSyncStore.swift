#if os(macOS)
import Foundation

/// Workspace 同步的 UI 薄壳——只做「读配置 → 起 `SyncEngine` → 转发结果给
/// `@Published`」，不把 Task 6/7 已经落好的编排/判定逻辑再搬一份进来（那些纪律
/// 全在 `SyncEngine`/`ProjectSyncService`/`WorkspaceRepoService` 里，这一层只转发）。
///
/// - workspace root / remote 从 `UserDefaults` 读（key 见 `rootKey`/`remoteKey`）——
///   Task 9 的设置 sheet 负责写这两个 key，这里只读，没配置时 `makeEngine()` 返回
///   `nil`，UI 据此显示引导占位（本 task 只放占位文案，接线是 Task 9 的事）。
/// - `machineId` 用 `DeviceIdentity.current`（安装级稳定 UUID，同侧栏机器分组同一份
///   身份）。
/// - 每个 `scan*`/`run*` 内部起一个 `Task.detached` 去跑 `SyncEngine`（它是同步、
///   可能耗时的 git 操作),完成后 `await MainActor.run` 回主线程发布；`executeUp`/
///   `executeDown` 的 `progress` 回调是**同步**闭包、且在 detached 的非隔离上下文里
///   触发,用 `Task { @MainActor in ... }`（本文件里其它同步→主线程转发的既有写法,
///   如 `CrewSessionRunner`）把 receipt 送回 `@Published receipts`——**发布整份
///   累积快照,不是逐条 append**：progress 回调把新 receipt 追加到一个只在这个
///   detached task 内部串行读写的局部数组,每次都把这个数组的当前完整拷贝丢进
///   `Task { @MainActor in self.receipts = snapshot }`。这样 UI 仍然在跑的过程中
///   逐项刷新,同时不依赖"多个各自独立的 unstructured `Task { @MainActor }` 会
///   按创建顺序执行"这条没有语言层面硬保证的假设——即便被乱序执行,每一份快照
///   都是当时那一刻的完整、正确顺序前缀,不会像"读 `self.receipts` 现值 →
///   append → 写回"那样因为乱序丢失别的 task 已经写入的元素。收尾 `finish(_:)`
///   额外再赋一次完整 `result` 兜底,保证最终态一定正确。
/// - `planDirection` 记录当前 `planItems` 是哪个方向扫出来的,`runUp`/`runDown`
///   开头各自 `guard planDirection == .up/.down`——防止"上行 plan 驱动下行执行"
///   这类跨方向误执行在**结构上**不可能发生,不只靠 view 层切方向时记得清空。
@MainActor
final class WorkspaceSyncStore: ObservableObject {
    /// 当前阶段。`done(ok:failed:)` 是这一轮 `runUp`/`runDown` 收尾后的汇总——
    /// `ok`/`failed` 如实来自 receipts 的 outcome 计数,不做整体成功/失败的二元
    /// 判断(同 `SyncEngine` 自己的纪律)。
    enum Phase: Equatable {
        case idle
        case scanning
        case syncing
        case done(ok: Int, failed: Int)
    }

    /// 当前 `planItems` 是哪个方向扫出来的——`nil` = 还没扫过 / 已被 `resetPlan()`
    /// 清空。`runUp`/`runDown` 各自 guard 这个值,让"用上行的 plan 驱动下行执行"
    /// (或反过来)在**结构上不可能**,不依赖 view 层的 UI 纪律(比如切换方向 Picker
    /// 忘了清空、或者未来别的入口调用 `runUp`/`runDown` 时漏了检查)。只在
    /// `scanUp`/`scanDown` **成功**时才和 `planItems` 一起写(同一个 `MainActor.run`
    /// 块内),失败时两者都保持旧值——不会出现"`planDirection` 已经翻新、`planItems`
    /// 还是旧方向的"这种不一致中间态。
    @Published private(set) var planDirection: Direction?
    @Published var planItems: [SyncPlanItem] = []
    @Published var receipts: [SyncReceipt] = []
    @Published var phase: Phase = .idle
    /// `planDown` 第 0 步（`WorkspaceRepoService.syncDown` 刷新 manifest 本身）的
    /// 回执——单独一个 `@Published`,不混进 `planItems`(那是 `.project`/
    /// `.workspaceRepo` 项),UI 顶部单独展示"这次刷新 manifest 成没成"。
    @Published var workspaceReceipt: SyncReceipt?
    /// 配置级错误（`layout.loadProjects()` 读不了目录等）——`SyncEngine` 的
    /// `throws` 边界原样转发到这里,UI 如实展示,不吞。
    @Published var lastError: String?

    /// 同步方向——`Direction` 挂在 store 上(而不是 view 私有枚举的重复定义),
    /// 让 `planDirection` 与 view 的方向切换 Picker 共用同一套词汇,不会出现两边
    /// 各自一份 enum、靠人工保持同步的漂移风险。
    enum Direction: String, CaseIterable, Identifiable, Equatable, Sendable {
        case up, down
        var id: String { rawValue }
    }

    static let rootDefaultsKey = "pendingcrew.workspace.root"
    static let remoteDefaultsKey = "pendingcrew.workspace.remote"

    /// 是否已配置 workspace root——UI 据此决定显示正常操作面板还是引导占位。
    /// 占位文案指向 Task 9 的设置 sheet(本 task 还没接线,只放文案)。
    var isConfigured: Bool { Self.workspaceRoot() != nil }

    static func workspaceRoot() -> URL? {
        guard let raw = UserDefaults.standard.string(forKey: rootDefaultsKey),
              !raw.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return URL(fileURLWithPath: (raw as NSString).expandingTildeInPath)
    }

    static func workspaceRemote() -> String? {
        UserDefaults.standard.string(forKey: remoteDefaultsKey)
    }

    private func makeEngine() -> SyncEngine? {
        guard let root = Self.workspaceRoot() else { return nil }
        return SyncEngine(layout: WorkspaceRepoLayout(root: root), machineId: DeviceIdentity.current)
    }

    // MARK: - 首次设置

    /// 首次设置(Task 9)——`WorkspaceSetupSheet` 的唯一入口:
    /// 1. `WorkspaceRepoService.ensure(at:remoteURL:name:)` 落地 workspace 仓库
    ///    (已存在直接返回 / clone / 从零 scaffold,三分支见该方法文档)。
    /// 2. `MachineRegistration.register` 登记本机(`machines/<DeviceIdentity.current>.toml`,
    ///    displayName = `DeviceIdentity.displayName`)。
    /// 3. `WorkspaceRepoService.syncUp` 推一次——把 `ensure`/`register` 落的内容
    ///    (首次 scaffold 提交 + 本机注册)尽快同步出去,不留在本地干等下一次
    ///    手动同步。
    /// 4. **只有 ensure/register 抛错才算 setup 失败**——`syncUp` 的结果无论是
    ///    `.skipped`(没配 remote,纯本地模式合法)还是 `.failed`(配了 remote
    ///    但首推没成,比如网络问题)都不影响 setup 本身的成败:workspace 仓库已经
    ///    在本机落地好了,只是"有没有推出去"是另一件事,回执照实存进
    ///    `receipts` 让用户看到,他可以之后在主视图里手动再同步一次。
    ///    只有 `ensure`/`register` 失败(半配置状态——目录/git 层面就没建好)
    ///    才不写 defaults,不能让 UI 显示"已配置"但实际仓库都没建起来。
    ///
    /// 跑在 `Task.detached`(git 操作可能阻塞),完成后 `MainActor.run` 回主线程
    /// 发布——同 `scanUp`/`runUp` 的既有模式。`completion` 在成功/失败两条路径
    /// 都会调用一次(总在 `MainActor.run` 内部、状态已经落地之后),给调用方
    /// (`WorkspaceSetupSheet`)一个"这轮 setup 跑完了,可以收起 busy 态"的信号
    /// ——`phase`/`lastError` 本身不够,因为两条路径都会把 `phase` 收尾成
    /// `.idle`,sheet 没法只靠 `phase` 变化区分"跑完了"和"还没开始";sheet
    /// 自行检查 `lastError == nil` 判断是否该 dismiss。
    func setup(root: URL, remoteURL: String?, completion: (() -> Void)? = nil) {
        phase = .scanning
        lastError = nil
        let machineId = DeviceIdentity.current
        let displayName = DeviceIdentity.displayName
        let name = root.lastPathComponent
        Task.detached { [weak self] in
            do {
                let layout = try WorkspaceRepoService.ensure(at: root, remoteURL: remoteURL, name: name)
                try MachineRegistration.register(layout: layout, machineId: machineId, displayName: displayName)

                let receipt = WorkspaceRepoService.syncUp(
                    layout: layout, message: "chore: register machine \(machineId)")

                await MainActor.run {
                    // `completion?()` 挪到 `guard let self` 之外调用——self 已释放时
                    // (理论上 store 生命周期够长不会发生,但不靠这个假设)也不能静默
                    // 吞掉调用方等待的"这轮 setup 跑完了"信号,和 catch 分支的无条件
                    // 调用对称。
                    if let self {
                        UserDefaults.standard.set(root.path, forKey: Self.rootDefaultsKey)
                        if let remoteURL, !remoteURL.trimmingCharacters(in: .whitespaces).isEmpty {
                            UserDefaults.standard.set(remoteURL, forKey: Self.remoteDefaultsKey)
                        } else {
                            UserDefaults.standard.removeObject(forKey: Self.remoteDefaultsKey)
                        }
                        self.receipts = [receipt]
                        self.workspaceReceipt = receipt
                        self.phase = .idle
                    }
                    completion?()
                }
            } catch {
                await MainActor.run {
                    // ensure/register 抛错 → 半配置状态,defaults 一个字都不写。
                    self?.lastError = "\(error)"
                    self?.phase = .idle
                    completion?()
                }
            }
        }
    }

    // MARK: - 上行

    /// 扫一遍「现在有什么需要上行」——只读,不改任何东西。
    func scanUp() {
        guard let engine = makeEngine() else { return }
        phase = .scanning
        lastError = nil
        Task.detached { [weak self] in
            do {
                let items = try engine.planUp(fetchFirst: true)
                await MainActor.run {
                    guard let self else { return }
                    self.planItems = items
                    self.receipts = []
                    self.workspaceReceipt = nil
                    self.planDirection = .up
                    self.phase = .idle
                }
            } catch {
                await MainActor.run {
                    self?.lastError = "\(error)"
                    self?.phase = .idle
                }
            }
        }
    }

    /// 执行一次上行——用当前 `planItems`(上一次 `scanUp` 的结果)驱动
    /// `executeUp`。`wipCommit` 透传给引擎(dirty 项是否允许自动 commit)。
    /// `guard planDirection == .up` 挡住"当前 plan 其实是下行扫出来的"这种
    /// 误执行——即便 view 层因为某种疏漏没在切方向时清空 plan,这里也不会拿着
    /// 一份下行 plan 跑上行执行。
    func runUp(wipCommit: Bool) {
        guard let engine = makeEngine(), planDirection == .up else { return }
        let items = planItems
        phase = .syncing
        receipts = []
        lastError = nil
        Task.detached { [weak self] in
            // 累积在这个 detached task 自己的局部变量里(同一个 task 内串行
            // 触发 progress,没有并发写)——每次发布**整份快照**而不是对
            // `self.receipts` 做"读现有值 → append → 写回",因为后者的读/写
            // 分别发生在各自独立的 `Task { @MainActor in }` 里,多个这样的
            // unstructured task 之间的执行顺序不是 Swift 语言层面的硬保证;
            // 一旦被乱序执行,"读旧值 append" 这种写法会丢别的 task 已经写
            // 进去的元素。发布不可变的完整前缀快照没有这个问题——乱序时最多
            // 是 UI 暂时显示一份较短的历史快照,最终(以及每一份快照本身)
            // 都不会丢元素,元素也天然保序(都是同一份 `acc` 在不同时刻的拷贝)。
            var acc: [SyncReceipt] = []
            do {
                let result = try engine.executeUp(items: items, wipCommit: wipCommit) { receipt in
                    acc.append(receipt)
                    let snapshot = acc
                    Task { @MainActor in self?.receipts = snapshot }
                }
                await MainActor.run { self?.finish(result) }
            } catch {
                await MainActor.run {
                    self?.lastError = "\(error)"
                    self?.phase = .idle
                }
            }
        }
    }

    // MARK: - 下行

    /// 扫一遍「现在需要从远端收敛什么到本机」——同时把第 0 步的 workspace 刷新
    /// 回执单独发布出去。
    func scanDown() {
        guard let engine = makeEngine() else { return }
        phase = .scanning
        lastError = nil
        Task.detached { [weak self] in
            do {
                let (workspaceReceipt, items) = try engine.planDown()
                await MainActor.run {
                    guard let self else { return }
                    self.workspaceReceipt = workspaceReceipt
                    self.planItems = items
                    self.receipts = []
                    self.planDirection = .down
                    self.phase = .idle
                }
            } catch {
                await MainActor.run {
                    self?.lastError = "\(error)"
                    self?.phase = .idle
                }
            }
        }
    }

    /// 执行一次下行——同 `runUp`,用当前 `planItems` 驱动 `executeDown`;
    /// `guard planDirection == .down` 同 `runUp` 的镜像纪律。
    func runDown() {
        guard let engine = makeEngine(), planDirection == .down else { return }
        let items = planItems
        phase = .syncing
        receipts = []
        lastError = nil
        Task.detached { [weak self] in
            // 同 `runUp`:局部累积 + 整份快照发布,理由见那边的注释。
            var acc: [SyncReceipt] = []
            do {
                let result = try engine.executeDown(items: items) { receipt in
                    acc.append(receipt)
                    let snapshot = acc
                    Task { @MainActor in self?.receipts = snapshot }
                }
                await MainActor.run { self?.finish(result) }
            } catch {
                await MainActor.run {
                    self?.lastError = "\(error)"
                    self?.phase = .idle
                }
            }
        }
    }

    /// 切换同步方向(或任何需要清空当前 plan 状态)时调用——把 `planItems`/
    /// `receipts`/`workspaceReceipt`/`planDirection`/`phase`/`lastError` 一并
    /// 清空,避免"UI 已经切到下行,列表却还挂着上行 plan"这种视觉+状态双重污染。
    /// 这是 view 层的第一道防线;`runUp`/`runDown` 的 `planDirection` guard 是
    /// 第二道(即便这里漏调,执行也不会跑错方向)。
    func resetPlan() {
        planItems = []
        receipts = []
        workspaceReceipt = nil
        planDirection = nil
        phase = .idle
        lastError = nil
    }

    // MARK: - 收尾

    /// 数 receipts 的 ✓/✗ 汇总——`.uploaded`/`.upToDate`/`.pulled`/`.skipped` 算
    /// 成功侧(skipped 是"有意不做",不是出错);`.failed` 才计入失败,如实呈现,
    /// 不做整体布尔判断。
    private func finish(_ result: [SyncReceipt]) {
        // 收尾再赋一次全量 `result`(而不是信任最后一次快照 `Task { @MainActor }`
        // 已经落地)——这个赋值发生在 `await MainActor.run` 里,在 `executeUp`/
        // `executeDown` 同步返回之后,是这次调用里最后一个进 MainActor 队列的
        // 动作,保证界面收尾时 `receipts` 一定是完整、正确顺序的最终结果,不
        // 依赖"进度快照 task 全部先于收尾 task 执行完"这条本来就只是实践惯例、
        // 没有语言层面硬保证的假设。
        receipts = result
        var ok = 0
        var failed = 0
        for receipt in result {
            switch receipt.outcome {
            case .uploaded, .upToDate, .pulled, .skipped:
                ok += 1
            case .failed:
                failed += 1
            }
        }
        phase = .done(ok: ok, failed: failed)
    }
}
#endif
