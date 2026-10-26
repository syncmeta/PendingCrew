import Foundation
import Network
import Security

/// Always TLS1.3 mutual authentication; no PSK or local-Human network fallback.
enum CrewdRemoteTransport {
    @MainActor static func connector(id: UUID, pairing: CrewdRemotePairing, expectedReceipt: CrewdRemoteReceipt) -> CrewdWorkspace.Connector {
        { client in
            let epoch=client.connectionEpoch
            let plan = try await pairing.connection(id)
            try Task.checkCancellation()
            guard client.connectionEpoch == epoch else {throw CancellationError()}
            guard plan.receipt == expectedReceipt else {throw CrewdRemoteError.wrongIdentity}
            let invitation = plan.invitation, receipt = plan.receipt, parameters = plan.parameters
            try await client.connect(host: invitation.endpoint.host, port: invitation.endpoint.port, parameters: parameters, expectedRemote: CrewdRemoteBindingExpectation(authorityID: receipt.authorityId, dbID: receipt.dbId, grantID: receipt.grantId, grantRevision: receipt.grantRevision),expectedEpoch:epoch)
        }
    }
}
