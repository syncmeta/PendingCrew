#if os(macOS)
import Foundation

/// Local-machine UI state, next to subscription detection. No updater is ever
/// called by start/refresh/timer; only the explicit confirmation action can mutate.
@MainActor
final class AgentCLIVersionCenter: ObservableObject {
    static let shared = AgentCLIVersionCenter()
    @Published private(set) var installations: [LocalCodingAgentKind: AgentCLIInstallation] = [:]
    @Published private(set) var errors: [LocalCodingAgentKind: String] = [:]
    @Published private(set) var results: [LocalCodingAgentKind: String] = [:]
    @Published private(set) var busy: Set<LocalCodingAgentKind> = []
    /// #188 的显式缺失/安装状态。它不是登录状态：整个流不读取 token 或认证缓存。
    @Published private(set) var codexProvisioningState: CodexCLIProvisioningState?
    private var timer: Timer?
    private let service: AgentCLIMaintenanceService
    private let codexProvisioning: CodexCLIProvisioningService

    init(service: AgentCLIMaintenanceService = .init(),
         codexProvisioning: CodexCLIProvisioningService = .init()) {
        self.service = service
        self.codexProvisioning = codexProvisioning
    }

    func start() {
        guard timer == nil else { return }
        Task { await refresh() }
        timer = Timer.scheduledTimer(withTimeInterval: 600, repeats: true) { [weak self] _ in
            Task { @MainActor in await self?.refresh() }
        }
    }

    func refresh() async {
        async let c: () = refresh(.claudeCode)
        async let x: () = refresh(.codex)
        _ = await (c, x)
    }

    func refresh(_ kind: LocalCodingAgentKind) async {
        guard busy.insert(kind).inserted else { return }
        defer { busy.remove(kind) }
        if kind == .codex {
            let state = codexProvisioning.currentState()
            codexProvisioningState = state
            if case .missing = state {
                installations[kind] = nil
                errors[kind] = nil
                results[kind] = nil
                return
            }
        }
        let service = service
        let result = await Task.detached(priority: .utility) { () -> Result<AgentCLIInstallation, Error> in
            Result {
                let lease = try AgentCLIMaintenanceLease.acquire(kind, exclusive: false, directory: service.lockDirectory)
                defer { withExtendedLifetime(lease) {} }
                return try service.inspect(kind)
            }
        }.value
        switch result {
        case let .success(value):
            installations[kind] = value
            errors[kind] = nil
            if kind == .codex {
                // `--version` only proves the fixed managed binary is present.  Do not
                // erase the explicit authentication-unknown boundary on the timer's
                // normal inspect path, and do not read cached auth to guess at it.
                if let managed = CodexCLIProvisioningService.managedInstallation(for: value) {
                    codexProvisioningState = .installedNeedsManualSignIn(managed)
                } else {
                    codexProvisioningState = .ready
                }
            }
        case let .failure(error):
            errors[kind] = error.localizedDescription
            if kind == .codex { codexProvisioningState = .failed(error.localizedDescription) }
        }
    }

    /// 第一次点击：只返回固定的、不可由 UI 文本替换字段的动作；不会启动 npm。
    @discardableResult
    func prepareCodexInstall() -> CodexCLIInstallAction? {
        guard busy.insert(.codex).inserted else { return nil }
        defer { busy.remove(.codex) }
        errors[.codex] = nil
        do {
            let action = try codexProvisioning.prepareOfficialInstall()
            codexProvisioningState = .awaitingSecondConfirmation(action)
            return action
        } catch {
            errors[.codex] = error.localizedDescription
            codexProvisioningState = .failed(error.localizedDescription)
            return nil
        }
    }

    /// 仅由确认对话框的第二次点击调用。成功也只证明二进制；认证仍由用户的原生
    /// Codex 登录流程负责，且本方法绝不启动 app-server 或 agent。
    func installCodex(_ action: CodexCLIInstallAction) async {
        guard busy.insert(.codex).inserted else { return }
        errors[.codex] = nil
        results[.codex] = nil
        codexProvisioningState = .installing(action)
        let provisioning = codexProvisioning
        let result = await Task.detached(priority: .userInitiated) {
            Result { try provisioning.installConfirmed(action) }
        }.value
        switch result {
        case let .success(installed):
            codexProvisioningState = .installedNeedsManualSignIn(installed)
            results[.codex] = "已验证受管 Codex CLI \(installed.version)。PendingCrew 没有读取登录状态；请由你自行完成 Codex 原生登录后再创建 session。"
        case let .failure(error):
            codexProvisioningState = .failed(error.localizedDescription)
            errors[.codex] = error.localizedDescription
        }
        busy.remove(.codex)
    }

    enum Action {
        case update(target: String)
        case rollback(release: String)
        case doctor
    }

    func perform(_ action: Action, installation: AgentCLIInstallation) async {
        let kind = installation.kind
        guard busy.insert(kind).inserted else { return }
        errors[kind] = nil
        results[kind] = "正在执行，请等待结果…"
        let service = service
        let result = await Task.detached(priority: .userInitiated) { () -> Result<String, Error> in
            Result {
                switch action {
                case let .update(target): return try service.update(installation, target: target)
                case let .rollback(release): return try service.rollback(installation, release: release)
                case .doctor:
                    let lease = try AgentCLIMaintenanceLease.acquire(kind, exclusive: false, directory: service.lockDirectory)
                    defer { withExtendedLifetime(lease) {} }
                    return try service.doctor(kind)
                }
            }
        }.value
        switch result {
        case let .success(text): results[kind] = text
        case let .failure(error): errors[kind] = error.localizedDescription; results[kind] = nil
        }
        busy.remove(kind)
        // Do not clear the maintenance failure with a successful read-only probe.
        let operationError = errors[kind]
        await refresh(kind)
        if let operationError { errors[kind] = operationError }
    }
}
#endif
