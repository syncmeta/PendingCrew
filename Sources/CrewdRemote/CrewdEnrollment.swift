import Foundation
import CryptoKit
import Security

/// Owns enrollment across sheets/windows. Before each possible mutation, its
/// exact key, request and phase are durable. Reopen never resumes a mutation.
@CrewdCredentialActor final class CrewdEnrollment {
    nonisolated static let shared=CrewdEnrollment()
    typealias RecoveryScope = @Sendable () async -> Bool
    typealias Call = @Sendable (String,Data,CrewdEnrollmentLink) async throws -> Data
    private var injectedVault:CrewdEnrollmentStoring?
    private var vault:CrewdEnrollmentStoring {if let injectedVault{return injectedVault};let v=CrewdEnrollmentVault.application();injectedVault=v;return v}
    private let call:Call
    private let pairing:CrewdRemotePairing
    private var running=Set<UUID>()
    nonisolated init(){call={try await CrewdEnrollmentTransport.call($0,$1,link:$2)};pairing = .shared}
    init(vault:CrewdEnrollmentStoring,pairing:CrewdRemotePairing,call:@escaping Call){injectedVault=vault;self.pairing=pairing;self.call=call}
    nonisolated static func preview(_ text:String)throws->CrewdEnrollmentLink {
        let link=try CrewdEnrollmentLink.decode(text)
        let certificate=try CrewdRemoteCertificate.decode(link.server.certificateDerBase64)
        let (_,pin)=try CrewdRemoteCertificate.validate(certificate,role:.server)
        guard pin==link.server.spkiSha256 else{throw CrewdRemoteError.wrongIdentity}
        return link
    }
    func removePairAttempt(_ pair:CrewdRemoteStoredPair,enrollmentId:String)throws {
        // Deleting an enrolled computer also deletes the private recovery copy;
        // querying a retained attempt cannot silently restore forgotten keys.
        let records=try vault.all()
        guard let record=records.first(where:{$0.id==pair.id}) else{return}
        guard case .recipient(let a)=record,a.link.enrollmentId==enrollmentId,
              a.privateKey==pair.privateKey,a.clientCertificate==pair.clientCertificate,
              a.response==pair.response,a.invitation==pair.invitation,
              (a.accepted==nil || a.accepted?.receipt==pair.receipt),!running.contains(a.id) else{throw CrewdRemoteError.wrongIdentity}
        try vault.remove(a.id)
    }
    func attempts()throws->[CrewdEnrollmentAttempt] {
        try vault.all().compactMap{if case .recipient(let v)=$0{return v};return nil}
            .sorted{$0.requestId<$1.requestId}
    }
    /// Explicit confirmation creates one original identity, or reopens the same
    /// existing enrollment; a second window cannot mint a replacement key.
    func prepare(_ link:CrewdEnrollmentLink,deviceLabel:String)throws->CrewdEnrollmentAttempt {
        _ = try Self.preview(link.encoded())
        if let original=try attempts().first(where:{$0.link.enrollmentId==link.enrollmentId && $0.link.server.authorityId==link.server.authorityId && $0.link.server.dbId==link.server.dbId}) {
            guard original.link==link else{throw CrewdRemoteError.wrongIdentity};return original
        }
        guard let expiry=CrewdRemoteText.date(link.expiresAt),expiry>Date(),expiry.timeIntervalSinceNow<=600 else{throw CrewdRemoteError.expiredInvitation}
        let label=deviceLabel.trimmingCharacters(in:.whitespacesAndNewlines)
        guard !label.isEmpty,label.utf8.count<=256,!CrewdRemoteText.containsControlScalar(label) else{throw CrewdRemoteError.invalidExchange}
        let key=try CrewdRemoteCertificate.generate()
        let (_,pin)=try CrewdRemoteCertificate.validate(key.certificate,role:.client)
        let attempt=CrewdEnrollmentAttempt(id:UUID(),link:link,deviceLabel:label,privateKey:key.privateKey,clientCertificate:key.certificate,clientPin:pin)
        try vault.save(.recipient(attempt));return attempt
    }
    private func read(_ id:UUID)throws->CrewdEnrollmentAttempt {
        guard case .recipient(let value)=try vault.read(id) else{throw CrewdRemoteError.wrongIdentity};return value
    }
    func begin(_ id:UUID,canRestore:@escaping RecoveryScope = {false})async throws->CrewdEnrollmentAttempt {
        guard running.insert(id).inserted else{throw CrewdRemoteError.invalidExchange};defer{running.remove(id)}
        var a=try read(id);guard a.phase == .prepared else{return a}
        guard let expiry=CrewdRemoteText.date(a.link.expiresAt),expiry>Date() else{throw CrewdRemoteError.expiredInvitation}
        let params=CrewdEnrollmentBegin(enrollmentId:a.link.enrollmentId,requestId:a.requestId,token:a.link.token,clientCertificateDerBase64:a.clientCertificate.base64EncodedString(),clientSpkiSha256:a.clientPin,deviceLabel:a.deviceLabel)
        let bytes=try JSONEncoder().encode(params)
        a.phase = .beginUnknown;try vault.save(.recipient(a))
        do {
            let result=try JSONDecoder().decode(CrewdEnrollmentBegan.self,from:await call("enrollment.begin",bytes,a.link))
            guard result.requestId==a.requestId else{throw CrewdRemoteError.wrongIdentity}
            try adoptChallenge(result.invitation,into:&a)
            try vault.save(.recipient(a));return a
        } catch {
            // Only this transport's proof of no handoff permits manual reuse.
            // Closing/switching the UI, cancellation alone, or a newer record does not.
            if let failure=error as? CrewdEnrollmentTransportFailure,
               failure.request == .begin,!failure.requestWasHandedToSend,await canRestore(),
               let current=try? read(id),current.phase == .beginUnknown,
               current.requestId==a.requestId,current.link==a.link,
               current.privateKey==a.privateKey,current.clientCertificate==a.clientCertificate,
               current.clientPin==a.clientPin,current.invitation==a.invitation,
               current.response==a.response,current.commitAttempted==a.commitAttempted {
                a.phase = .prepared
                try vault.save(.recipient(a))
            }
            throw error
        }
    }
    /// Only an explicit confirmation of an already durable, verified challenge
    /// reaches commit. Unknown commit is never submitted a second time.
    func commit(_ id:UUID,canRestore:@escaping RecoveryScope = {false})async throws->CrewdEnrollmentAttempt {
        guard running.insert(id).inserted else{throw CrewdRemoteError.invalidExchange};defer{running.remove(id)}
        var a=try read(id)
        guard a.phase == .challengeReady,!a.commitAttempted,let response=a.response else{return a}
        a.phase = .commitUnknown;a.commitAttempted=true
        let bytes=try JSONEncoder().encode(CrewdEnrollmentCommit(enrollmentId:a.link.enrollmentId,requestId:a.requestId,responseEnvelope:response))
        try vault.save(.recipient(a))
        do {
            let result=try JSONDecoder().decode(CrewdRemoteAcceptResult.self,from:await call("enrollment.commit",bytes,a.link))
            try accept(result,into:&a,currentGrant:result.grant)
            try vault.save(.recipient(a));return a
        } catch {
            // Only this transport's proof of no handoff permits manual reuse.
            // Closing/switching the UI, cancellation alone, or a newer record does not.
            if let failure=error as? CrewdEnrollmentTransportFailure,
               failure.request == .commit,!failure.requestWasHandedToSend,await canRestore(),
               let current=try? read(id),current.phase == .commitUnknown,
               current.requestId==a.requestId,current.link==a.link,
               current.privateKey==a.privateKey,current.clientCertificate==a.clientCertificate,
               current.clientPin==a.clientPin,current.invitation==a.invitation,
               current.response==a.response,current.commitAttempted==a.commitAttempted {
                a.phase = .challengeReady;a.commitAttempted=false
                try vault.save(.recipient(a))
            }
            throw error
        }
    }
    func lookup(_ id:UUID)async throws->CrewdEnrollmentAttempt {
        guard running.insert(id).inserted else{throw CrewdRemoteError.invalidExchange};defer{running.remove(id)}
        var a=try read(id)
        let digest=try a.invitation.map { envelope -> String in
            guard let payload=Data(base64Encoded:envelope.payload) else{throw CrewdRemoteError.invalidExchange}
            return SHA256.hash(data:payload).map{String(format:"%02x",$0)}.joined()
        }
        let proof=try CrewdRemoteCertificate.sign(CrewdEnrollmentProof(enrollmentId:a.link.enrollmentId,requestId:a.requestId,authorityId:a.link.server.authorityId,dbId:a.link.server.dbId,clientCertificateDerBase64:a.clientCertificate.base64EncodedString(),clientSpkiSha256:a.clientPin,beginDigest:digest),privateKey:a.privateKey)
        let params=CrewdEnrollmentLookup(enrollmentId:a.link.enrollmentId,requestId:a.requestId,proof:proof)
        let result=try JSONDecoder().decode(CrewdEnrollmentFound.self,from:await call("enrollment.lookup",JSONEncoder().encode(params),a.link))
        guard result.requestId==a.requestId else{throw CrewdRemoteError.wrongIdentity}
        switch result.state {
        case .pending:
            guard let envelope=result.pendingInvitation,result.accepted==nil,result.grantState==nil else{throw CrewdRemoteError.invalidExchange}
            try adoptChallenge(envelope,into:&a)
            // Once commit might have been sent, a pending observation is not a
            // replay permit. It only refreshes the original challenge.
            if a.commitAttempted{a.phase = .commitUnknown}
        case .accepted:
            guard let accepted=result.accepted,let grant=result.grantState,result.pendingInvitation==nil else{throw CrewdRemoteError.invalidExchange}
            // A lost begin response may recover accepted with no local challenge.
            // Server normally returns the original pending invitation only before
            // commit; no device could have committed without saving it first.
            try accept(accepted,into:&a,currentGrant:grant)
        case .revoked:
            guard let accepted=result.accepted,let grant=result.grantState,result.pendingInvitation==nil,
                  let invitation=a.invitation,let response=a.response else{throw CrewdRemoteError.invalidExchange}
            try validateGrant(accepted,attempt:a,currentGrant:grant)
            guard grant.revokedAt != nil else{throw CrewdRemoteError.wrongIdentity}
            _ = try pairing.verifyEnrollmentReceipt(CrewdRemoteStoredPair(id:a.id,invitation:invitation,response:response,privateKey:a.privateKey,clientCertificate:a.clientCertificate,receipt:accepted.receipt,enrollmentId:a.link.enrollmentId),grantId:accepted.grant.id,grantRevision:accepted.grant.revision)
            a.phase = .revoked
        case .expired:
            guard let envelope=result.pendingInvitation,result.accepted==nil,result.grantState==nil else{throw CrewdRemoteError.invalidExchange}
            try adoptChallenge(envelope,into:&a);a.phase = .expired
        case .notFound:
            guard result.pendingInvitation==nil,result.accepted==nil,result.grantState==nil else{throw CrewdRemoteError.invalidExchange}
            a.phase = .notFound // Still no permission to retry.
        }
        try vault.save(.recipient(a));return a
    }
    private func adoptChallenge(_ envelope:CrewdRemoteEnvelope,into a:inout CrewdEnrollmentAttempt)throws {
        let invite=try pairing.invitation(envelope,requireFresh:false)
        guard invite.invitationId==a.link.enrollmentId,invite.server==a.link.server,invite.endpoint==a.link.endpoint,invite.computerLabel==a.link.computerLabel,
              invite.expiresAt==a.link.expiresAt else{throw CrewdRemoteError.wrongIdentity}
        if let old=a.invitation{guard old==envelope else{throw CrewdRemoteError.wrongIdentity}}
        let response=try CrewdRemoteCertificate.sign(CrewdRemoteResponse(invitationId:invite.invitationId,authorityId:invite.server.authorityId,dbId:invite.server.dbId,challenge:invite.challenge,clientCertificateDerBase64:a.clientCertificate.base64EncodedString(),clientSpkiSha256:a.clientPin,deviceLabel:a.deviceLabel),privateKey:a.privateKey)
        a.invitation=envelope
        // Retain the original signature bytes across lookup/reopen.
        if a.response==nil{a.response=response}
        a.phase = .challengeReady
    }
    private func validateGrant(_ result:CrewdRemoteAcceptResult,attempt a:CrewdEnrollmentAttempt,currentGrant:CrewdRemoteGrant)throws {
        guard result.requestId==a.requestId,result.grant.scope=="ownerWorkspace",currentGrant.scope==result.grant.scope,
              currentGrant.deviceLabel==result.grant.deviceLabel,currentGrant.createdAt==result.grant.createdAt,
              result.grant.clientSpkiSha256==a.clientPin,
              result.grant.deviceLabel==a.deviceLabel,result.grant.id==currentGrant.id,
              currentGrant.clientSpkiSha256==a.clientPin,CrewdRemoteText.uuid(currentGrant.id),
              CrewdRemoteText.revision(currentGrant.revision) else{throw CrewdRemoteError.wrongIdentity}
    }
    private func accept(_ result:CrewdRemoteAcceptResult,into a:inout CrewdEnrollmentAttempt,currentGrant:CrewdRemoteGrant)throws {
        try validateGrant(result,attempt:a,currentGrant:currentGrant)
        guard currentGrant.revokedAt==nil,result.grant.revokedAt==nil,currentGrant.revision==result.grant.revision,
              let invitation=a.invitation,let response=a.response else{throw CrewdRemoteError.wrongIdentity}
        let receipt=try pairing.adoptEnrollment(CrewdRemoteStoredPair(id:a.id,invitation:invitation,response:response,privateKey:a.privateKey,clientCertificate:a.clientCertificate,receipt:result.receipt,enrollmentId:a.link.enrollmentId),grantId:result.grant.id,grantRevision:result.grant.revision)
        guard receipt.grantId==result.grant.id,receipt.grantRevision==result.grant.revision else{throw CrewdRemoteError.wrongIdentity}
        a.accepted=result;a.phase = .accepted
    }
}

extension CrewdEnrollment {
    struct IssuerSummary:Sendable {
        let id:UUID;let requestId:String;let enrollmentId:String
        let endpoint:CrewdRemoteEndpoint
        let link:String?
        let unknown:Bool
        let prepared:Bool
    }
    private func issuerSummary(_ a:CrewdHostEnrollmentAttempt)throws->IssuerSummary {
        let text=try a.created.map{created in
            try CrewdEnrollmentLink(version:1,enrollmentId:created.enrollmentId,server:created.server,endpoint:created.endpoint,expiresAt:created.expiresAt,computerLabel:created.computerLabel,token:a.token).encoded()
        }
        return .init(id:a.id,requestId:a.requestId,enrollmentId:a.enrollmentId,endpoint:a.endpoint,link:text,unknown:a.submitted && a.created==nil,prepared:!a.submitted)
    }
    func issuerOriginal(server:CrewdRemotePublicIdentity)throws->IssuerSummary? {
        let records=try vault.all().compactMap{record->CrewdHostEnrollmentAttempt? in if case .issuer(let a)=record,a.server==server{return a};return nil}
        let unresolved=records.first{$0.submitted && $0.created==nil}
        let current=records.first{a in a.created.flatMap{CrewdRemoteText.date($0.expiresAt)}.map{$0>Date()} ?? !a.submitted}
        return try (unresolved ?? current).map{try issuerSummary($0)}
    }
    func createHostLink(server:CrewdRemotePublicIdentity,endpoint:CrewdRemoteEndpoint,label:String,renewConfirmed:Bool=false,
                        send:@Sendable (String,Data)async throws->Data)async throws->IssuerSummary {
        // Current Local Human, capability and managed floor are independently
        // checked by the original server. UI callers also fence current scope.
        if let original=try issuerOriginal(server:server){
            // A deliberate second-device link may replace a known creation in
            // the presentation, never a prepared/Unknown original mutation.
            if !renewConfirmed || original.link==nil{return original}
        }
        guard endpoint.valid,!label.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,label.utf8.count<=256,
              !CrewdRemoteText.containsControlScalar(label) else{throw CrewdRemoteError.invalidExchange}
        var token=Data(count:32)
        let status=token.withUnsafeMutableBytes{SecRandomCopyBytes(kSecRandomDefault,32,$0.baseAddress!)}
        guard status==errSecSuccess else{throw CrewdRemoteError.unavailableCredential}
        let a=CrewdHostEnrollmentAttempt(id:UUID(),enrollmentId:UUID().uuidString.lowercased(),server:server,endpoint:endpoint,computerLabel:label,token:CrewdEnrollmentLink.url64(token))
        try vault.save(.issuer(a))
        return try await submitHostOriginal(a.id,server:server,send:send)
    }
    func submitHostOriginal(_ id:UUID,server:CrewdRemotePublicIdentity,
                            send:@Sendable (String,Data)async throws->Data)async throws->IssuerSummary {
        guard running.insert(id).inserted else{throw CrewdRemoteError.invalidExchange};defer{running.remove(id)}
        guard case .issuer(var a)=try vault.read(id),a.server==server else{throw CrewdRemoteError.wrongIdentity}
        guard !a.submitted else{return try issuerSummary(a)}
        guard let token=CrewdEnrollmentLink.decodeURL64(a.token),token.count==32 else{throw CrewdRemoteError.invalidExchange}
        let tokenHash=SHA256.hash(data:token).map{String(format:"%02x",$0)}.joined()
        let params=CrewdHostEnrollmentCreate(requestId:a.requestId,enrollmentId:a.enrollmentId,tokenHash:tokenHash,computerLabel:a.computerLabel,endpoint:a.endpoint)
        let bytes=try JSONEncoder().encode(params)
        a.submitted=true;try vault.save(.issuer(a))
        let result=try JSONDecoder().decode(CrewdHostEnrollmentCreated.self,from:await send("remote.enrollmentCreate",bytes))
        try validateCreated(result,original:a);a.created=result;try vault.save(.issuer(a));return try issuerSummary(a)
    }
    func lookupHostLink(_ id:UUID,server:CrewdRemotePublicIdentity,
                        send:@Sendable (String,Data)async throws->Data)async throws->IssuerSummary {
        guard running.insert(id).inserted else{throw CrewdRemoteError.invalidExchange};defer{running.remove(id)}
        guard case .issuer(var a)=try vault.read(id),a.server==server else{throw CrewdRemoteError.wrongIdentity}
        let params=["requestId":a.requestId,"enrollmentId":a.enrollmentId]
        let result=try JSONDecoder().decode(CrewdHostEnrollmentFound.self,from:await send("remote.enrollmentLookup",JSONEncoder().encode(params)))
        if result.state == .created {
            guard let created=result.created else{throw CrewdRemoteError.invalidExchange}
            try validateCreated(created,original:a);a.created=created;try vault.save(.issuer(a))
        }else if result.created != nil{throw CrewdRemoteError.invalidExchange}
        return try issuerSummary(a) // notFound never unlocks a replacement token.
    }
    private func validateCreated(_ created:CrewdHostEnrollmentCreated,original:CrewdHostEnrollmentAttempt)throws {
        guard created.requestId==original.requestId,created.enrollmentId==original.enrollmentId,
              created.server==original.server,created.endpoint==original.endpoint,created.computerLabel==original.computerLabel,
              CrewdRemoteText.date(created.expiresAt) != nil else{throw CrewdRemoteError.wrongIdentity}
        _ = try Self.preview(CrewdEnrollmentLink(version:1,enrollmentId:created.enrollmentId,server:created.server,endpoint:created.endpoint,expiresAt:created.expiresAt,computerLabel:created.computerLabel,token:original.token).encoded())
    }
}

struct CrewdEnrollmentSummary:Identifiable,Sendable {
    let id:UUID;let name:String;let deviceLabel:String;let endpoint:CrewdRemoteEndpoint;let pin:String
    let phase:CrewdEnrollmentAttempt.Phase
    var requestId:String{id.uuidString.lowercased()}
}
extension CrewdEnrollmentAttempt {
    var summary:CrewdEnrollmentSummary{.init(id:id,name:link.computerLabel,deviceLabel:deviceLabel,endpoint:link.endpoint,pin:link.server.spkiSha256,phase:phase)}
}
extension CrewdEnrollment {
    func summaries()throws->[CrewdEnrollmentSummary]{try attempts().map(\.summary)}
    func confirm(_ link:CrewdEnrollmentLink,deviceLabel:String,canRestore:@escaping RecoveryScope = {false})async throws->CrewdEnrollmentSummary{
        let original=try prepare(link,deviceLabel:deviceLabel)
        return try await begin(original.id,canRestore:canRestore).summary
    }
    func confirmChallenge(_ id:UUID,canRestore:@escaping RecoveryScope = {false})async throws->CrewdEnrollmentSummary{try await commit(id,canRestore:canRestore).summary}
    func queryOriginal(_ id:UUID)async throws->CrewdEnrollmentSummary{try await lookup(id).summary}
}
