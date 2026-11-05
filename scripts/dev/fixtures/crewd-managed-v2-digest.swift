import Foundation

@main struct ManagedV2DigestProof {
    typealias Intent=CrewdManagedV2Intent
    struct Entry:Decodable {let key:String;let value:Node}
    struct Node:Decodable {
        let type:String
        let text:String?
        let value:String?
        let bool:Bool?
        let values:[Node]?
        let entries:[Entry]?
        func typed()throws->Intent.Value {
            switch type {
            case "null":return .null
            case "bool":guard let bool else {throw Intent.Failure.invalid};return .bool(bool)
            case "string":guard let text else {throw Intent.Failure.invalid};return .string(text)
            case "unsigned":guard let value,let n=UInt64(value),String(n)==value else {throw Intent.Failure.invalid};return .unsigned(n)
            case "realBits":guard let value,value.count==16,let bits=UInt64(value,radix:16) else {throw Intent.Failure.invalid};return .real(Double(bitPattern:bits))
            case "array":guard let values else {throw Intent.Failure.invalid};return .array(try values.map{try $0.typed()})
            case "object":guard let entries else {throw Intent.Failure.invalid};return .object(try entries.map{($0.key,try $0.value.typed())})
            default:throw Intent.Failure.invalid
            }
        }
    }
    struct Normalized:Decodable {
        enum Original {case table(CrewdManagedTableV2Wire.Submit),manual(CrewdManagedTableV2Wire.ManualSubmit)}
        let original:Original?
        let bytes:Data?
        let digest:String?
        enum Keys:String,CodingKey {case method}
        init(from decoder:Decoder)throws {
            let c=try decoder.container(keyedBy:Keys.self)
            let method=try c.decodeIfPresent(String.self,forKey:.method)
            if method==CrewdManagedTableV2Wire.submit {
                let input=try CrewdManagedTableV2Wire.Submit(from:decoder)
                guard input.operation.valid else {throw Intent.Failure.invalid}
                bytes=try Intent.bytes(input.intent);digest=try input.digest()
                original = .table(input)
            } else if method==CrewdManagedTableV2Wire.linkSubmit {
                let input=try CrewdManagedTableV2Wire.ManualSubmit(from:decoder)
                guard input.operation.valid else {throw Intent.Failure.invalid}
                bytes=try Intent.bytes(input.intent);digest=try input.digest()
                original = .manual(input)
            } else {bytes=nil;digest=nil;original=nil}
        }
    }
    struct Vector:Decodable {let name:String;let node:Node;let normalized:Normalized;let canonicalHex:String;let sha256:String}
    @MainActor static func main()throws {
        let plus=try Intent.digest(.real(0.0)),minus=try Intent.digest(.real(-0.0))
        precondition(plus != minus)
        let nfc="\u{e9}",nfd="e\u{301}"
        precondition(nfc==nfd && !nfc.utf8.elementsEqual(nfd.utf8))
        let nfcDigest=try Intent.digest(.string(nfc)),nfdDigest=try Intent.digest(.string(nfd));precondition(nfcDigest != nfdDigest)
        _=try Intent.bytes(.object([(nfc,.null),(nfd,.null)]))
        do {_=try Intent.bytes(.object([("same",.null),("same",.bool(true))]));fatalError("duplicate key accepted")}catch{}
        do {_=try Intent.bytes(.real(.infinity));fatalError("nonfinite accepted")}catch{}
        do {_=try Intent.bytes(.real(.nan));fatalError("NaN accepted")}catch{}
        var count=0
        if CommandLine.arguments.count==2 {
            let vectors=try JSONDecoder().decode([Vector].self,from:Data(contentsOf:URL(fileURLWithPath:CommandLine.arguments[1])))
            for vector in vectors {
                let value=try vector.node.typed()
                let bytes=try Intent.bytes(value).map{String(format:"%02x",$0)}.joined()
                precondition(bytes==vector.canonicalHex,"canonical bytes mismatch: "+vector.name)
                let digest=try Intent.digest(value);precondition(digest==vector.sha256,"digest mismatch: "+vector.name)
                if let normalized=vector.normalized.bytes {
                    precondition(normalized.map{String(format:"%02x",$0)}.joined()==vector.canonicalHex,"typed DTO normalization mismatch: "+vector.name)
                    precondition(vector.normalized.digest==vector.sha256)
                }
                if let original=vector.normalized.original {
                    let transported:String
                    switch original {
                    case .table(let input):transported=try CrewdKnowledgeStore.decode(CrewdKnowledgeStore.encode(input),CrewdManagedTableV2Wire.Submit.self).digest()
                    case .manual(let input):transported=try CrewdKnowledgeStore.decode(CrewdKnowledgeStore.encode(input),CrewdManagedTableV2Wire.ManualSubmit.self).digest()
                    }
                    precondition(transported==vector.sha256,"actual transport normalization mismatch: "+vector.name)
                }
                count+=1
            }
        }
        print("PASS typed intent UTF8 bytes/order/unsigned/IEEE754 ±0/nonfinite/duplicate key; shared golden vectors: \(count)")
    }
}
