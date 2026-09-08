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
    private var timer: Timer?
    private let service: AgentCLIMaintenanceService

    init(service: AgentCLIMaintenanceService = .init()) { self.service = service }

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
        let service = service
        let result = await Task.detached(priority: .utility) { () -> Result<AgentCLIInstallation, Error> in
            Result {
                let lease = try AgentCLIMaintenanceLease.acquire(kind, exclusive: false, directory: service.lockDirectory)
                defer { withExtendedLifetime(lease) {} }
                return try service.inspect(kind)
            }
        }.value
        switch result {
        case let .success(value): installations[kind] = value; errors[kind] = nil
        case let .failure(error): errors[kind] = error.localizedDescription
        }
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
