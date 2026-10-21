import Foundation
import Network
import Security

/// Always TLS1.3 mutual authentication; no PSK or local-Human network fallback.
enum CrewdRemoteTransport {
    @MainActor static func connector(id: UUID, pairing: CrewdRemotePairing) -> CrewdWorkspace.Connector {
        { client in
            let plan = try await pairing.connection(id)
            let invitation = plan.invitation, receipt = plan.receipt, parameters = plan.parameters
            try await client.connect(host: invitation.endpoint.host, port: invitation.endpoint.port, parameters: parameters, expectedRemote: CrewdRemoteBindingExpectation(authorityID: receipt.authorityId, dbID: receipt.dbId, grantID: receipt.grantId, grantRevision: receipt.grantRevision))
        }
    }
}
