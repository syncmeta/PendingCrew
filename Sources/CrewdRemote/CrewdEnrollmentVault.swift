import Foundation
import Security

struct CrewdEnrollmentAttempt: Codable, Sendable, Identifiable {
    enum Phase:String,Codable,Sendable {case prepared,beginUnknown,challengeReady,commitUnknown,accepted,expired,revoked,notFound}
    let id:UUID
    let link:CrewdEnrollmentLink
    let deviceLabel:String
    let privateKey:Data
    let clientCertificate:Data
    let clientPin:String
    var phase:Phase = .prepared
    var invitation:CrewdRemoteEnvelope?
    var response:CrewdRemoteEnvelope?
    var accepted:CrewdRemoteAcceptResult?
    var commitAttempted=false
    var requestId:String{id.uuidString.lowercased()}
}
struct CrewdHostEnrollmentAttempt: Codable, Sendable, Identifiable {
    let id:UUID
    let enrollmentId:String
    let server:CrewdRemotePublicIdentity
    let endpoint:CrewdRemoteEndpoint
    let computerLabel:String
    let token:String
    var submitted=false
    var created:CrewdHostEnrollmentCreated?
    var requestId:String{id.uuidString.lowercased()}
}
/// Only B's two closed record shapes live here. ThisDeviceOnly and a separate
/// service keep raw one-use tokens away from Cloud, preferences and old pairs.
@CrewdCredentialActor protocol CrewdEnrollmentStoring {
    func all()throws->[CrewdEnrollmentVault.Record]
    func read(_ id:UUID)throws->CrewdEnrollmentVault.Record
    func save(_ record:CrewdEnrollmentVault.Record)throws
    func remove(_ id:UUID)throws
}
@CrewdCredentialActor final class CrewdEnrollmentVault:CrewdEnrollmentStoring {
    enum Record:Codable {case recipient(CrewdEnrollmentAttempt),issuer(CrewdHostEnrollmentAttempt)
        var id:UUID {switch self{case .recipient(let v):return v.id;case .issuer(let v):return v.id}}
    }
    private let service:String?
    private let memory:Bool
    private var values:[UUID:Record]=[:]
    init(service:String?,memory:Bool=false,records:[Record]=[]){self.service=service;self.memory=memory;if memory{for record in records{values[record.id]=record}}}
    static func application()->CrewdEnrollmentVault {
        #if PENDINGCREW_NATIVE_CANDIDATE || PENDINGCREW_DELIVERY_VALIDATION || PENDINGCREW_KNOWLEDGE_VALIDATION || (PENDINGCREW_LOCAL_MAC && !PENDINGCREW_REMOTE_PRODUCT)
        return .init(service:nil)
        #else
        if ProcessInfo.processInfo.environment["PENDINGCREW_ISOLATED_CREDENTIALS"] == "memory" {return .init(service:nil,memory:true)}
        return .init(service:CrewdRemoteCredentials.keychainService(bundleIdentifier:Bundle.main.bundleIdentifier).map{$0+".enrollment.v1"})
        #endif
    }
    private func query(_ id:UUID?=nil)throws->[String:Any] {
        guard let service else{throw CrewdRemoteError.unavailableCredential}
        var q:[String:Any]=[kSecClass as String:kSecClassGenericPassword,kSecAttrService as String:service,kSecAttrSynchronizable as String:false]
        if let id {q[kSecAttrAccount as String]=id.uuidString.lowercased()};return q
    }
    func all()throws->[Record] {
        if memory{return Array(values.values)}
        var q=try query();q[kSecReturnAttributes as String]=true;q[kSecMatchLimit as String]=kSecMatchLimitAll
        var raw:CFTypeRef?;let status=SecItemCopyMatching(q as CFDictionary,&raw)
        if status==errSecItemNotFound{return []}
        guard status==errSecSuccess,let attributes=raw as? [[String:Any]],attributes.count<=128 else{throw CrewdRemoteError.unavailableCredential}
        return try attributes.map { item in
            guard let account=item[kSecAttrAccount as String] as? String,let id=UUID(uuidString:account),id.uuidString.lowercased()==account else{throw CrewdRemoteError.unavailableCredential}
            return try read(id)
        }
    }
    func read(_ id:UUID)throws->Record {
        if memory{guard let value=values[id] else{throw CrewdRemoteError.unavailableCredential};return value}
        var q=try query(id);q[kSecReturnData as String]=true;q[kSecMatchLimit as String]=kSecMatchLimitOne
        var raw:CFTypeRef?;guard SecItemCopyMatching(q as CFDictionary,&raw)==errSecSuccess,
            let data=raw as? Data,data.count<=524288,let record=try? JSONDecoder().decode(Record.self,from:data),record.id==id else{throw CrewdRemoteError.unavailableCredential}
        return record
    }
    func remove(_ id:UUID)throws {
        if memory{values.removeValue(forKey:id);return}
        let q=try query(id);let status=SecItemDelete(q as CFDictionary)
        guard status==errSecSuccess || status==errSecItemNotFound else{throw CrewdRemoteError.storageFailure}
    }
    func save(_ record:Record)throws {
        let data=try JSONEncoder().encode(record);guard data.count<=524288 else{throw CrewdRemoteError.storageFailure}
        if memory{guard values[record.id] != nil || values.count<128 else{throw CrewdRemoteError.storageFailure};values[record.id]=record;return}
        let q=try query(record.id)
        let fields:[String:Any]=[kSecValueData as String:data,kSecAttrAccessible as String:kSecAttrAccessibleWhenUnlockedThisDeviceOnly]
        let status=SecItemUpdate(q as CFDictionary,fields as CFDictionary)
        if status==errSecSuccess{return}
        guard status==errSecItemNotFound else{throw CrewdRemoteError.storageFailure}
        guard try all().count<128,SecItemAdd(q.merging(fields){_,new in new} as CFDictionary,nil)==errSecSuccess else{throw CrewdRemoteError.storageFailure}
    }
}
