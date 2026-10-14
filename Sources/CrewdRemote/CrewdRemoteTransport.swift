import Foundation
import Network
import Security

/// Always TLS1.3 mutual authentication; no PSK or local-Human network fallback.
enum CrewdRemoteTransport {
    @MainActor static func connector(id: UUID, pairing: CrewdRemotePairing) -> CrewdWorkspace.Connector {
        { client in
            let (pair, invitation, receipt) = try pairing.paired(id)
            let identity = try CrewdRemoteCertificate.securityIdentity(privateKey: pair.privateKey, certificate: pair.clientCertificate)
            guard let protocolIdentity = sec_identity_create(identity) else { throw CrewdRemoteError.unavailableCredential }
            let options = NWProtocolTLS.Options()
            sec_protocol_options_set_min_tls_protocol_version(options.securityProtocolOptions, .TLSv13)
            sec_protocol_options_set_max_tls_protocol_version(options.securityProtocolOptions, .TLSv13)
            sec_protocol_options_set_tls_tickets_enabled(options.securityProtocolOptions, false)
            sec_protocol_options_set_tls_resumption_enabled(options.securityProtocolOptions, false)
            sec_protocol_options_set_peer_authentication_required(options.securityProtocolOptions, true)
            sec_protocol_options_set_local_identity(options.securityProtocolOptions, protocolIdentity)
            let expectedPin = receipt.serverSpkiSha256
            sec_protocol_options_set_verify_block(options.securityProtocolOptions, { _, trust, complete in
                let actualTrust = sec_trust_copy_ref(trust).takeRetainedValue()
                guard let chain = SecTrustCopyCertificateChain(actualTrust) as? [SecCertificate], chain.count == 1 else { complete(false); return }
                do {
                    let (_, pin) = try CrewdRemoteCertificate.validate(SecCertificateCopyData(chain[0]) as Data, role: .server)
                    complete(pin == expectedPin)
                } catch { complete(false) }
            }, DispatchQueue(label: "pendingcrew.crewd.remote.certificate"))
            let parameters = NWParameters(tls: options, tcp: NWProtocolTCP.Options())
            parameters.allowLocalEndpointReuse = false
            try await client.connect(host: invitation.endpoint.host, port: invitation.endpoint.port, parameters: parameters, expectedRemote: CrewdRemoteBindingExpectation(authorityID: receipt.authorityId, dbID: receipt.dbId, grantID: receipt.grantId, grantRevision: receipt.grantRevision))
        }
    }
}
