import Foundation

/// PendingCrew URL scheme 解析。
///
/// 形式（spec v2 §11.3）：
///   pendingcrew://crew/<crew-id>
///   pendingcrew://crew/<crew-id>/session/<session-id>
///   pendingcrew://machine/<machine-id>  (v1.1)
struct PendingCrewDeepLinkRoute: Equatable {
    enum Target: Equatable {
        case crew(id: String, sessionId: String?)
        case machine(id: String)
    }

    let target: Target

    init?(url: URL) {
        guard url.scheme == "pendingcrew" else { return nil }

        var parts: [String] = []
        if let host = url.host(percentEncoded: false), !host.isEmpty {
            parts.append(host)
        }
        parts.append(contentsOf: url.pathComponents.filter { $0 != "/" })

        guard let head = parts.first else { return nil }

        switch head {
        case "crew":
            guard parts.count >= 2 else { return nil }
            let crewId = parts[1]
            let sessionId: String?
            if parts.count >= 4, parts[2] == "session" {
                sessionId = parts[3]
            } else {
                sessionId = nil
            }
            target = .crew(id: crewId, sessionId: sessionId)
        case "machine":
            guard parts.count >= 2 else { return nil }
            target = .machine(id: parts[1])
        default:
            return nil
        }
    }
}
