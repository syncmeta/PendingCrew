import Foundation
import CoreFoundation

/// Secret-bearing link. Never print, log, persist in preferences or sync to Cloud.
struct CrewdEnrollmentLink: Codable, Equatable, Sendable {
    let version: UInt32
    let enrollmentId: String
    let server: CrewdRemotePublicIdentity
    let endpoint: CrewdRemoteEndpoint
    let expiresAt: String
    let computerLabel: String
    let token: String
    static let prefix = "pendingcrew://enroll/v1/"
    static func decode(_ text: String) throws -> Self {
        guard text.utf8.count <= 8192, text.hasPrefix(prefix),
              let data = decodeURL64(String(text.dropFirst(prefix.count))) else { throw CrewdRemoteError.invalidExchange }
        var scanner = EnrollmentJSONKeys(bytes: Array(data)); try scanner.check()
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String:Any],
              Set(object.keys) == Set(["version","enrollmentId","server","endpoint","expiresAt","computerLabel","token"]),
              let server=object["server"] as? [String:Any], Set(server.keys) == Set(["authorityId","dbId","certificateDerBase64","spkiSha256"]),
              let endpoint=object["endpoint"] as? [String:Any], Set(endpoint.keys) == Set(["host","port"]) else {throw CrewdRemoteError.invalidExchange}
        let link = try JSONDecoder().decode(Self.self, from:data)
        guard link.version == 1, CrewdRemoteText.uuid(link.enrollmentId), CrewdRemoteText.uuid(link.server.authorityId),
              CrewdRemoteText.databaseID(link.server.dbId), CrewdRemoteText.pin(link.server.spkiSha256),
              link.endpoint.valid, !link.computerLabel.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,
              link.computerLabel.utf8.count <= 256, !CrewdRemoteText.containsControlScalar(link.computerLabel),
              CrewdRemoteText.date(link.expiresAt) != nil,
              let token=decodeURL64(link.token),token.count == 32,
              let certificate=Data(base64Encoded:link.server.certificateDerBase64), !certificate.isEmpty,certificate.count<=65536 else {throw CrewdRemoteError.invalidExchange}
        return link
    }
    func encoded() throws -> String {
        let text=Self.prefix+Self.url64(try JSONEncoder().encode(self))
        _ = try Self.decode(text)
        return text
    }
    static func url64(_ bytes:Data)->String {bytes.base64EncodedString().replacingOccurrences(of:"+",with:"-").replacingOccurrences(of:"/",with:"_").replacingOccurrences(of:"=",with:"")}
    static func decodeURL64(_ text:String)->Data? {
        guard !text.isEmpty, text.utf8.allSatisfy({(65...90).contains($0)||(97...122).contains($0)||(48...57).contains($0)||$0==45||$0==95}) else{return nil}
        var base=text.replacingOccurrences(of:"-",with:"+").replacingOccurrences(of:"_",with:"/")
        base += String(repeating:"=",count:(4-base.utf8.count%4)%4)
        guard let bytes=Data(base64Encoded:base),url64(bytes)==text else{return nil}
        return bytes
    }
}
/// JSONDecoder accepts duplicate keys. This bounded wire-only scanner rejects
/// them (including escaped aliases) before the normal typed decoder runs.
struct EnrollmentJSONKeys {
    let bytes:[UInt8]; var i=0
    mutating func check() throws {try value(0);space();guard i==bytes.count else{throw CrewdRemoteError.invalidExchange}}
    mutating func space(){while i<bytes.count && [UInt8(32),9,10,13].contains(bytes[i]){i+=1}}
    mutating func take(_ byte:UInt8)->Bool {space();guard i<bytes.count,bytes[i]==byte else{return false};i+=1;return true}
    mutating func string() throws -> String {
        space();let start=i;guard take(34) else{throw CrewdRemoteError.invalidExchange}
        while i<bytes.count {
            let byte=bytes[i];i+=1
            if byte==92 {guard i<bytes.count else{throw CrewdRemoteError.invalidExchange};i+=1}
            else if byte==34{return try JSONDecoder().decode(String.self,from:Data(bytes[start..<i]))}
        }
        throw CrewdRemoteError.invalidExchange
    }
    mutating func value(_ depth:Int) throws {
        guard depth<16 else{throw CrewdRemoteError.invalidExchange};space()
        guard i<bytes.count else{throw CrewdRemoteError.invalidExchange}
        if bytes[i]==123 {
            i+=1;var keys=Set<String>();if take(125){return}
            repeat {let key=try string();guard keys.insert(key).inserted,take(58) else{throw CrewdRemoteError.invalidExchange};try value(depth+1);if take(125){return}} while take(44)
            throw CrewdRemoteError.invalidExchange
        } else if bytes[i]==91 {
            i+=1;if take(93){return};repeat {try value(depth+1);if take(93){return}} while take(44)
            throw CrewdRemoteError.invalidExchange
        } else if bytes[i]==34 {_ = try string()}
        else {let start=i;while i<bytes.count && ![UInt8(32),9,10,13,44,93,125].contains(bytes[i]){i+=1};guard i>start else{throw CrewdRemoteError.invalidExchange}}
    }
}
struct CrewdEnrollmentBegin: Codable, Sendable {
    let enrollmentId:String;let requestId:String;let token:String
    let clientCertificateDerBase64:String;let clientSpkiSha256:String;let deviceLabel:String
}
struct CrewdEnrollmentBegan: Codable, Sendable {let requestId:String;let invitation:CrewdRemoteEnvelope}
struct CrewdEnrollmentCommit: Codable, Sendable {let enrollmentId:String;let requestId:String;let responseEnvelope:CrewdRemoteEnvelope}
struct CrewdEnrollmentLookup: Codable, Sendable {let enrollmentId:String;let requestId:String;let proof:CrewdRemoteEnvelope}
struct CrewdEnrollmentProof: Encodable, Sendable {
    let format=1;let domain="pendingcrew.enrollment.lookup.v1"
    let enrollmentId:String;let requestId:String;let authorityId:String;let dbId:String
    let clientCertificateDerBase64:String;let clientSpkiSha256:String;let beginDigest:String?
    enum CodingKeys:String,CodingKey {case format,domain,enrollmentId,requestId,authorityId,dbId,clientCertificateDerBase64,clientSpkiSha256,beginDigest}
    func encode(to encoder:Encoder)throws {
        var c=encoder.container(keyedBy:CodingKeys.self)
        try c.encode(format,forKey:.format);try c.encode(domain,forKey:.domain)
        try c.encode(enrollmentId,forKey:.enrollmentId);try c.encode(requestId,forKey:.requestId)
        try c.encode(authorityId,forKey:.authorityId);try c.encode(dbId,forKey:.dbId)
        try c.encode(clientCertificateDerBase64,forKey:.clientCertificateDerBase64);try c.encode(clientSpkiSha256,forKey:.clientSpkiSha256)
        if let beginDigest {try c.encode(beginDigest,forKey:.beginDigest)}else{try c.encodeNil(forKey:.beginDigest)}
    }
}
struct CrewdEnrollmentFound: Decodable, Sendable {
    enum State:String,Decodable,Sendable {case pending,accepted,expired,revoked,notFound}
    let requestId:String;let state:State
    let pendingInvitation:CrewdRemoteEnvelope?;let accepted:CrewdRemoteAcceptResult?;let grantState:CrewdRemoteGrant?
}
struct CrewdHostEnrollmentCreate: Codable, Sendable {
    let requestId:String;let enrollmentId:String;let tokenHash:String;let computerLabel:String;let endpoint:CrewdRemoteEndpoint
}
struct CrewdHostEnrollmentCreated: Codable, Sendable {
    let requestId:String;let enrollmentId:String;let server:CrewdRemotePublicIdentity
    let endpoint:CrewdRemoteEndpoint;let expiresAt:String;let computerLabel:String
}
struct CrewdHostEnrollmentFound: Decodable, Sendable {
    enum State:String,Decodable,Sendable {case created,notFound}
    let state:State;let created:CrewdHostEnrollmentCreated?
}

/// Closed enrollment replies reject events, both outcomes, duplicate keys and
/// mismatched IDs before any durable phase can advance.
enum CrewdEnrollmentReply {
    private struct Header:Decodable{let id:UInt64}
    static func result(_ data:Data)throws->Data {
        guard data.count<=262144 else{throw CrewdRemoteError.invalidExchange}
        var scanner=EnrollmentJSONKeys(bytes:Array(data));try scanner.check()
        guard let object=try JSONSerialization.jsonObject(with:data) as? [String:Any],
              Set(object.keys)==Set(["id","result"]),
              let id=object["id"] as? NSNumber,CFGetTypeID(id) != CFBooleanGetTypeID(),
              try JSONDecoder().decode(Header.self,from:data).id==1,
              let result=object["result"] as? [String:Any] else{throw CrewdRemoteError.invalidExchange}
        return try JSONSerialization.data(withJSONObject:result)
    }
}
