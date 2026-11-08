import Foundation
import Network
import Security

/// Always TLS1.3 mutual authentication; no PSK or local-Human network fallback.
enum CrewdRemoteTransport {
    /// The original signed receipt and Security identity were qualified together
    /// by Pairing.selection. Reconnects use the original re-reading connector so
    /// a later explicit credential change is still checked before a new socket.
    @MainActor static func preparedConnector(id: UUID, pairing: CrewdRemotePairing, selection: CrewdRemotePairing.Selection) -> CrewdWorkspace.Connector {
        var prepared: CrewdRemotePairing.Connection? = selection.connection
        return { client in
            let epoch = client.connectionEpoch
            let plan: CrewdRemotePairing.Connection
            let revision: UUID
            if let first = prepared { plan = first; revision = selection.credentialRevision; prepared = nil }
            else {
                let value = try await CrewdCredentialReadBroker.shared.observe(.selection(pairing.observationID, selection.computer), owner: pairing, timeout: CrewdConnectionDeadlines().credential) {
                    .selection(try pairing.selection(selection.computer))
                }
                guard case .selection(let next) = value else { throw CrewdRemoteError.wrongIdentity }
                plan = next.connection; revision = next.credentialRevision
            }
            try Task.checkCancellation()
            guard client.connectionEpoch == epoch else { throw CancellationError() }
            guard revision == pairing.credentialRevision.current else { throw CrewdRemoteError.wrongIdentity }
            guard plan.receipt == selection.connection.receipt else { throw CrewdRemoteError.wrongIdentity }
            try await client.connect(host: plan.invitation.endpoint.host, port: plan.invitation.endpoint.port, parameters: plan.parameters,
                expectedRemote: CrewdRemoteBindingExpectation(authorityID: plan.receipt.authorityId, dbID: plan.receipt.dbId, grantID: plan.receipt.grantId, grantRevision: plan.receipt.grantRevision), expectedEpoch: epoch)
        }
    }
    @MainActor static func connector(id: UUID, pairing: CrewdRemotePairing, expectedReceipt: CrewdRemoteReceipt) -> CrewdWorkspace.Connector {
        { client in
            let epoch=client.connectionEpoch
            let revision = pairing.credentialRevision.current
            let value = try await CrewdCredentialReadBroker.shared.observe(.connection(pairing.observationID, id, expectedReceipt), owner: pairing, timeout: CrewdConnectionDeadlines().credential) {
                .connection(try pairing.connection(id))
            }
            guard case .connection(let plan) = value else { throw CrewdRemoteError.wrongIdentity }
            guard revision == pairing.credentialRevision.current else { throw CrewdRemoteError.wrongIdentity }
            try Task.checkCancellation()
            guard client.connectionEpoch == epoch else {throw CancellationError()}
            guard plan.receipt == expectedReceipt else {throw CrewdRemoteError.wrongIdentity}
            let invitation = plan.invitation, receipt = plan.receipt, parameters = plan.parameters
            try await client.connect(host: invitation.endpoint.host, port: invitation.endpoint.port, parameters: parameters, expectedRemote: CrewdRemoteBindingExpectation(authorityID: receipt.authorityId, dbID: receipt.dbId, grantID: receipt.grantId, grantRevision: receipt.grantRevision),expectedEpoch:epoch)
        }
    }
}
