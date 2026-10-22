import Foundation
import CryptoKit
import X509

actor EnrollmentFixtureServer {
    let key:Data;let link:CrewdEnrollmentLink
    var calls:[String]=[]
    var loseBegin=false;var loseCommit=false;var mismatch=false;var noResult=false;var badReceipt=false;var revoked=false
    var invitation:CrewdRemoteEnvelope?
    var accepted:CrewdRemoteAcceptResult?
    init(key:Data,link:CrewdEnrollmentLink){self.key=key;self.link=link}
    func set(_ mode:String){loseBegin=mode=="beginLoss";loseCommit=mode=="commitLoss";mismatch=mode=="mismatch";noResult=mode=="notFound";badReceipt=mode=="badReceipt";revoked=mode=="revoked"}
    func count(_ method:String)->Int{calls.filter{$0==method}.count}
    func call(_ method:String,_ bytes:Data,_ descriptor:CrewdEnrollmentLink) throws -> Data {
        precondition(descriptor==link);calls.append(method)
        if method=="enrollment.begin" {
            let p=try JSONDecoder().decode(CrewdEnrollmentBegin.self,from:bytes)
            precondition(p.enrollmentId==link.enrollmentId && p.token==link.token)
            let envelope=try CrewdRemoteCertificate.sign(CrewdRemoteInvitation(invitationId:link.enrollmentId,server:link.server,endpoint:link.endpoint,challenge:"original-challenge",expiresAt:link.expiresAt,computerLabel:link.computerLabel),privateKey:key)
            invitation=envelope
            if loseBegin{throw CrewdRemoteError.invalidExchange}
            return try JSONEncoder().encode(CrewdEnrollmentBegan(requestId:mismatch ? UUID().uuidString.lowercased():p.requestId,invitation:envelope))
        }
        if method=="enrollment.commit" {
            let p=try JSONDecoder().decode(CrewdEnrollmentCommit.self,from:bytes)
            let raw=try JSONDecoder().decode(CrewdRemoteResponse.self,from:Data(base64Encoded:p.responseEnvelope.payload)!)
            let certificate=try CrewdRemoteCertificate.decode(raw.clientCertificateDerBase64)
            let response=try CrewdRemoteCertificate.verify(p.responseEnvelope,certificate:certificate,role:.client,expectedPin:raw.clientSpkiSha256,as:CrewdRemoteResponse.self)
            precondition(response.challenge=="original-challenge")
            let grant=CrewdRemoteGrant(id:UUID().uuidString.lowercased(),scope:"ownerWorkspace",deviceLabel:response.deviceLabel,clientSpkiSha256:response.clientSpkiSha256,revision:"1",createdAt:ISO8601DateFormatter().string(from:Date()),revokedAt:nil,revokeNote:nil)
            let receipt=try CrewdRemoteCertificate.sign(CrewdRemoteReceipt(invitationId:link.enrollmentId,grantId:grant.id,authorityId:link.server.authorityId,dbId:link.server.dbId,serverSpkiSha256:link.server.spkiSha256,clientSpkiSha256:response.clientSpkiSha256,grantRevision:grant.revision,acceptedAt:grant.createdAt),privateKey:key)
            let returnedGrant=badReceipt ? CrewdRemoteGrant(id:UUID().uuidString.lowercased(),scope:grant.scope,deviceLabel:grant.deviceLabel,clientSpkiSha256:grant.clientSpkiSha256,revision:grant.revision,createdAt:grant.createdAt,revokedAt:nil,revokeNote:nil):grant
            let result=CrewdRemoteAcceptResult(requestId:p.requestId,reused:false,receipt:receipt,grant:returnedGrant);accepted=result
            if loseCommit{throw CrewdRemoteError.invalidExchange}
            return try JSONEncoder().encode(result)
        }
        precondition(method=="enrollment.lookup")
        let p=try JSONDecoder().decode(CrewdEnrollmentLookup.self,from:bytes)
        let object=try JSONSerialization.jsonObject(with:Data(base64Encoded:p.proof.payload)!) as! [String:Any]
        precondition(object["domain"] as? String == "pendingcrew.enrollment.lookup.v1")
        precondition(object["requestId"] as? String == p.requestId)
        let certificate=try CrewdRemoteCertificate.decode(object["clientCertificateDerBase64"] as! String)
        struct Proof:Decodable{let beginDigest:String?;let requestId:String}
        let verified=try CrewdRemoteCertificate.verify(p.proof,certificate:certificate,role:.client,expectedPin:object["clientSpkiSha256"] as! String,as:Proof.self)
        precondition(verified.requestId==p.requestId)
        if noResult{return try JSONSerialization.data(withJSONObject:["requestId":p.requestId,"state":"notFound"])}
        if let accepted {
            if let digest=verified.beginDigest {
                precondition(digest==SHA256.hash(data:Data(base64Encoded:invitation!.payload)!).map{String(format:"%02x",$0)}.joined())
            }
            let current=revoked ? CrewdRemoteGrant(id:accepted.grant.id,scope:accepted.grant.scope,deviceLabel:accepted.grant.deviceLabel,clientSpkiSha256:accepted.grant.clientSpkiSha256,revision:"2",createdAt:accepted.grant.createdAt,revokedAt:ISO8601DateFormatter().string(from:Date()),revokeNote:"synthetic revoke"):accepted.grant
            return try JSONSerialization.data(withJSONObject:["requestId":p.requestId,"state":revoked ? "revoked":"accepted","accepted":JSONSerialization.jsonObject(with:JSONEncoder().encode(accepted)),"grantState":JSONSerialization.jsonObject(with:JSONEncoder().encode(current))])
        }
        precondition(object["beginDigest"] is NSNull)
        return try JSONSerialization.data(withJSONObject:["requestId":p.requestId,"state":"pending","pendingInvitation":JSONSerialization.jsonObject(with:JSONEncoder().encode(invitation!))])
    }
}
@main struct EnrollmentLifecycleFixture {
    static func serverIdentity() throws -> (Data,CrewdRemotePublicIdentity) {
        let key=P256.Signing.PrivateKey();let name=try DistinguishedName{CommonName("synthetic enrollment fixture")}
        let certificate=try Certificate(version:.v3,serialNumber:.init(bytes:Array(repeating:1,count:16)),publicKey:.init(key.publicKey),notValidBefore:Date().addingTimeInterval(-60),notValidAfter:Date().addingTimeInterval(3600),issuer:name,subject:name,extensions:Certificate.Extensions{
            Critical(KeyUsage(digitalSignature:true));try ExtendedKeyUsage([.serverAuth]);Critical(BasicConstraints.notCertificateAuthority)
        },issuerPrivateKey:.init(key))
        let der=Data(try certificate.serializeAsPEM().derBytes)
        let (_,pin)=try CrewdRemoteCertificate.validate(der,role:.server)
        return(key.derRepresentation,.init(authorityId:UUID().uuidString.lowercased(),dbId:"source-database-not-a-uuid",certificateDerBase64:der.base64EncodedString(),spkiSha256:pin))
    }
    static func main()async throws {try await exercise()}
    @CrewdCredentialActor static func exercise()async throws {
        let (key,identity)=try serverIdentity()
        let link=CrewdEnrollmentLink(version:1,enrollmentId:UUID().uuidString.lowercased(),server:identity,endpoint:.init(host:"154.36.143.228",port:7423),expiresAt:ISO8601DateFormatter().string(from:Date().addingTimeInterval(300)),computerLabel:"synthetic",token:CrewdEnrollmentLink.url64(Data(repeating:7,count:32)))
        _ = try CrewdEnrollment.preview(link.encoded())
        if CommandLine.arguments.contains("--renew") {
            let vault=CrewdEnrollmentVault(service:nil,memory:true)
            let issuer=EnrollmentFixtureIssuer(server:identity)
            let service=CrewdEnrollment(vault:vault,pairing:CrewdRemotePairing(credentials:CrewdRemoteMemoryCredentials()),call:{_,_,_ in throw CrewdRemoteError.invalidExchange})
            do{_ = try await service.createHostLink(server:identity,endpoint:link.endpoint,label:"first",send:{try await issuer.call($0,$1)})}catch{}
            let original=try service.issuerOriginal(server:identity)!
            let known=try await service.lookupHostLink(original.id,server:identity,send:{try await issuer.call($0,$1)})
            precondition(known.link != nil)
            #if ENROLLMENT_FIXED_KNOWN_LINK
            do{_ = try await service.createHostLink(server:identity,endpoint:link.endpoint,label:"second",renewConfirmed:true,send:{try await issuer.call($0,$1)})}catch{}
            #else
            do{_ = try await service.createHostLink(server:identity,endpoint:link.endpoint,label:"second",send:{try await issuer.call($0,$1)})}catch{}
            #endif
            let next=try service.issuerOriginal(server:identity)!
            precondition(next.id != known.id && next.unknown,"confirmed one-use link was reused instead of an explicit new device link")
            #if ENROLLMENT_FIXED_KNOWN_LINK
            let unresolved=try await service.createHostLink(server:identity,endpoint:link.endpoint,label:"third",renewConfirmed:true,send:{try await issuer.call($0,$1)})
            precondition(unresolved.id==next.id && unresolved.unknown)
            let count=await issuer.creations;precondition(count==2,"renewal replayed an unresolved issuer mutation")
            #endif
            print("PASS explicit new link after confirmed receipt, unresolved replacement forbidden")
            return
        }
        for mode in ["normal","beginLoss","commitLoss","mismatch","notFound","badReceipt","revoked"] {
            let vault=CrewdEnrollmentVault(service:nil,memory:true)
            let credentials=CrewdRemoteMemoryCredentials()
            var removalOwner:CrewdEnrollment?
            let pairing=CrewdRemotePairing(credentials:credentials,enrollmentRemoval:{pair,id in try removalOwner!.removePairAttempt(pair,enrollmentId:id)})
            let server=EnrollmentFixtureServer(key:key,link:link);await server.set(mode)
            @CrewdCredentialActor func service(_ store:CrewdEnrollmentVault)->CrewdEnrollment{.init(vault:store,pairing:pairing,call:{try await server.call($0,$1,$2)})}
            let a=service(vault);let original=try a.prepare(link,deviceLabel:"A")
            let second=try a.prepare(link,deviceLabel:"B");precondition(second.id==original.id && second.privateKey==original.privateKey)
            do{_ = try await a.begin(original.id)}catch{precondition(mode=="beginLoss" || mode=="mismatch")}
            if ["normal","commitLoss","badReceipt","revoked"].contains(mode) {do{_ = try await a.commit(original.id)}catch{precondition(mode=="commitLoss" || mode=="badReceipt")}}
            // Reopen from exact encoded record bytes, not the coordinator's cache.
            let data=try JSONEncoder().encode(vault.all())
            let fresh=CrewdEnrollmentVault(service:nil,memory:true,records:try JSONDecoder().decode([CrewdEnrollmentVault.Record].self,from:data))
            let b=service(fresh);removalOwner=b
            do{_ = try await b.begin(original.id)}catch{let n=await server.count("enrollment.begin");precondition(n==1,"replayed original begin")}
            let count=await server.count("enrollment.begin");precondition(count==1)
            if mode=="beginLoss" {
                let p=try await b.lookup(original.id);precondition(p.phase == .challengeReady && p.id==original.id)
                _ = try await b.commit(original.id)
            }else if mode=="commitLoss" {
                _ = try await b.commit(original.id)
                let commits=await server.count("enrollment.commit");precondition(commits==1)
                let recovered=try await b.lookup(original.id);precondition(recovered.phase == .accepted)
            }else if mode=="notFound" {
                let p=try await b.lookup(original.id);precondition(p.phase == .notFound)
                _ = try await b.commit(original.id)
                let commits=await server.count("enrollment.commit");precondition(commits==0)
            }
            if mode=="normal" || mode=="beginLoss" || mode=="commitLoss" || mode=="revoked" {
                let receipt=try pairing.receipt(original.id);precondition(receipt.clientSpkiSha256==original.clientPin)
            }
            if mode=="badReceipt" {let pairs=try credentials.records();precondition(pairs.isEmpty)}
            if mode=="revoked" {let state=try await b.lookup(original.id);precondition(state.phase == .revoked);_ = try await b.commit(original.id);let n=await server.count("enrollment.commit");precondition(n==1)}
            if mode=="normal" {
                let summary=try pairing.summaries().first!;try pairing.forget(summary)
                let pairs=try credentials.records();let records=try fresh.all();precondition(pairs.isEmpty && records.isEmpty)
                do{_ = try await b.lookup(original.id);fatalError("forgotten key restored")}catch{}
            }
            print("PASS "+mode+" original key/request, durable phase, no automatic replay")
        }
        for failingPhase in [CrewdEnrollmentAttempt.Phase.prepared,.beginUnknown,.challengeReady,.commitUnknown,.accepted] {
            let storage=EnrollmentFixtureFailingVault();storage.fail=failingPhase
            let credentials=CrewdRemoteMemoryCredentials();let pairing=CrewdRemotePairing(credentials:credentials)
            let server=EnrollmentFixtureServer(key:key,link:link)
            let service=CrewdEnrollment(vault:storage,pairing:pairing,call:{try await server.call($0,$1,$2)})
            if failingPhase == .prepared {
                do{_ = try service.prepare(link,deviceLabel:"A");fatalError("save refusal ignored")}catch{}
                let values=try storage.all();precondition(values.isEmpty)
            }else {
                let original=try service.prepare(link,deviceLabel:"A")
                do{_ = try await service.begin(original.id);if failingPhase == .commitUnknown || failingPhase == .accepted {_ = try await service.commit(original.id)};fatalError("stage write refusal ignored")}catch{}
                guard case .recipient(let saved)=try storage.read(original.id) else{fatalError("wrong record")}
                if failingPhase == .beginUnknown {precondition(saved.phase == .prepared);let count=await server.count("enrollment.begin");precondition(count==0)}
                if failingPhase == .challengeReady {precondition(saved.phase == .beginUnknown)}
                if failingPhase == .commitUnknown {precondition(saved.phase == .challengeReady);let count=await server.count("enrollment.commit");precondition(count==0)}
                if failingPhase == .accepted {precondition(saved.phase == .commitUnknown);storage.fail=nil;let recovered=try await service.lookup(original.id);precondition(recovered.phase == .accepted)}
            }
            print("PASS storage refusal "+failingPhase.rawValue+" retains last durable original, no premature mutation")
        }
        let vault=CrewdEnrollmentVault(service:nil,memory:true)
        let issuer=EnrollmentFixtureIssuer(server:identity)
        let service=CrewdEnrollment(vault:vault,pairing:CrewdRemotePairing(credentials:CrewdRemoteMemoryCredentials()),call:{_,_,_ in throw CrewdRemoteError.invalidExchange})
        do {_ = try await service.createHostLink(server:identity,endpoint:link.endpoint,label:"local",send:{try await issuer.call($0,$1)});fatalError("expected original issuer reply loss")}catch{}
        let data=try JSONEncoder().encode(vault.all())
        let reopened=CrewdEnrollmentVault(service:nil,memory:true,records:try JSONDecoder().decode([CrewdEnrollmentVault.Record].self,from:data))
        let fresh=CrewdEnrollment(vault:reopened,pairing:CrewdRemotePairing(credentials:CrewdRemoteMemoryCredentials()),call:{_,_,_ in throw CrewdRemoteError.invalidExchange})
        let original=try await fresh.createHostLink(server:identity,endpoint:link.endpoint,label:"changed",send:{try await issuer.call($0,$1)})
        precondition(original.unknown)
        let recovered=try await fresh.lookupHostLink(original.id,server:identity,send:{try await issuer.call($0,$1)})
        let recoveredLink=try CrewdEnrollmentLink.decode(recovered.link!)
        await issuer.check(recoveredLink)
        print("PASS issuer lost reply/reopen/original lookup/raw-token hash without second creation")
    }
}

actor EnrollmentFixtureIssuer {
    let server:CrewdRemotePublicIdentity
    var params:CrewdHostEnrollmentCreate?
    var created:CrewdHostEnrollmentCreated?
    var creations=0
    init(server:CrewdRemotePublicIdentity){self.server=server}
    func call(_ method:String,_ bytes:Data)throws->Data {
        if method=="remote.enrollmentCreate" {
            creations+=1
            let p=try JSONDecoder().decode(CrewdHostEnrollmentCreate.self,from:bytes);params=p
            created=CrewdHostEnrollmentCreated(requestId:p.requestId,enrollmentId:p.enrollmentId,server:server,endpoint:p.endpoint,expiresAt:ISO8601DateFormatter().string(from:Date().addingTimeInterval(300)),computerLabel:p.computerLabel)
            throw CrewdRemoteError.invalidExchange // Committed, reply lost.
        }
        precondition(method=="remote.enrollmentLookup")
        let p=try JSONDecoder().decode([String:String].self,from:bytes)
        precondition(p["requestId"]==created?.requestId && p["enrollmentId"]==created?.enrollmentId)
        return try JSONSerialization.data(withJSONObject:["state":"created","created":JSONSerialization.jsonObject(with:JSONEncoder().encode(created!))])
    }
    func check(_ link:CrewdEnrollmentLink){
        precondition(creations==1)
        let raw=CrewdEnrollmentLink.decodeURL64(link.token)!
        precondition(params?.tokenHash==SHA256.hash(data:raw).map{String(format:"%02x",$0)}.joined())
        precondition(link.enrollmentId==params?.enrollmentId)
    }
}

@CrewdCredentialActor final class EnrollmentFixtureFailingVault:CrewdEnrollmentStoring {
    let underlying=CrewdEnrollmentVault(service:nil,memory:true)
    var fail:CrewdEnrollmentAttempt.Phase?
    var failInitial=false
    func all()throws->[CrewdEnrollmentVault.Record]{try underlying.all()}
    func read(_ id:UUID)throws->CrewdEnrollmentVault.Record{try underlying.read(id)}
    func remove(_ id:UUID)throws{try underlying.remove(id)}
    func save(_ record:CrewdEnrollmentVault.Record)throws {
        if case .recipient(let a)=record, (failInitial && a.phase == .prepared) || a.phase==fail {
            throw CrewdRemoteError.storageFailure
        }
        try underlying.save(record)
    }
}
