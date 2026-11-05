import Foundation
import CryptoKit

/// Versioned typed intent bytes. Never hash JSON encoder ordering or float spelling.
enum CrewdManagedV2Intent {
    enum Failure:Error {case invalid}
    indirect enum Value {
        case null, bool(Bool), string(String), unsigned(UInt64), real(Double)
        case array([Value]), object([(String,Value)])
    }
    static func bytes(_ value:Value)throws->Data {
        var output=Data("managed-v2-intent.v1".utf8);output.append(0)
        try append(value,to:&output);return output
    }
    static func digest(_ value:Value)throws->String {
        SHA256.hash(data:try bytes(value)).map{String(format:"%02x",$0)}.joined()
    }
    private static func integer(_ value:UInt64,to output:inout Data) {
        var big=value.bigEndian
        withUnsafeBytes(of:&big) {output.append(contentsOf:$0)}
    }
    private static func append(_ value:Value,to output:inout Data)throws {
        switch value {
        case .null:output.append(0x00)
        case .bool(false):output.append(0x01)
        case .bool(true):output.append(0x02)
        case .string(let text):
            let utf8=Data(text.utf8);output.append(0x10);integer(UInt64(utf8.count),to:&output);output.append(utf8)
        case .unsigned(let number):output.append(0x11);integer(number,to:&output)
        case .real(let number):
            guard number.isFinite else {throw Failure.invalid}
            output.append(0x12);integer(number.bitPattern,to:&output)
        case .array(let values):
            output.append(0x20);integer(UInt64(values.count),to:&output)
            for item in values {try append(item,to:&output)}
        case .object(let entries):
            guard Set(entries.map{Data($0.0.utf8)}).count==entries.count else {throw Failure.invalid}
            let sorted=entries.sorted{ $0.0.utf8.lexicographicallyPrecedes($1.0.utf8) }
            output.append(0x21);integer(UInt64(sorted.count),to:&output)
            for (key,value) in sorted {try append(.string(key),to:&output);try append(value,to:&output)}
        }
    }
}

/// Separate from V1 positional cells and closed V1 object codecs.
enum CrewdManagedTableV2Wire {
    typealias Intent=CrewdManagedV2Intent.Value
    typealias Scope=CrewdManagedDirectoryWire.Scope
    static let capability="managedTables.v2",linksCapability="managedLinks.v1",knowledgeCapability="managedLinks.knowledgeTargets.v1"
    static let tableGet="managedTable.v2.get",tableList="managedTable.v2.list",rows="managedTable.v2.rows",rowGet="managedTable.v2.row.get"
    static let submit="managedTable.v2.submit",lookup="managedTable.v2.request.get"
    static let links="managedLink.list",linkSubmit="managedLink.submit",linkLookup="managedLink.request.get"
    static let observation="managedTable.v2.observation",linkObservation="managedLink.observation"
    static func exact<T:Encodable>(_ a:T,_ b:T)->Bool {CrewdArtifactWire.exact(a,b)}
    static func number(_ text:String)->UInt64? {guard let n=UInt64(text),n<=UInt64(Int64.max),String(n)==text else {return nil};return n}
    static func positive(_ text:String)->Bool {number(text).map{$0>0} ?? false}
    static func fieldNumber(_ text:String)->UInt64? {guard text.hasPrefix("p") else {return nil};let s=String(text.dropFirst());guard let n=number(s),n>0 else {return nil};return n}
    static func uuid(_ text:String)->Bool {UUID(uuidString:text)?.uuidString.lowercased()==text}
    static func opaque(_ text:String,max:Int=4096)->Bool {!text.isEmpty && text.utf8.count<=max && !text.unicodeScalars.contains{CharacterSet.controlCharacters.contains($0)}}
    static func label(_ text:String,limit:Int,empty:Bool=false)->Bool {
        (empty || !text.isEmpty) && text.unicodeScalars.count<=limit && !text.unicodeScalars.contains{CharacterSet.controlCharacters.contains($0)} && text.utf8.elementsEqual(text.trimmingCharacters(in:.whitespacesAndNewlines).utf8)
    }
    static func unique(_ values:[String])->Bool {Set(values.map{Data($0.utf8)}).count==values.count}
    static func object(_ entries:(String,Intent)...)->Intent {.object(entries)}
    static func optional(_ value:String?)->Intent {value.map(Intent.string) ?? .null}
    static func scopeIntent(_ scope:Scope)->Intent {object(("dbId",.string(scope.dbId)),("restoreEpoch",.string(scope.restoreEpoch)))}

    enum TargetKind:String,Codable,CaseIterable,Identifiable {
        case work,requirement,ask,table,row,decision,risk,lesson,artifact,crew,session,channel,message,human,document,knowledge
        var id:Self {self}
        var title:String {switch self {case .work:"计划";case .requirement:"需求";case .ask:"人类事项";case .table:"组织表";case .row:"组织记录";case .decision:"决定";case .risk:"风险";case .lesson:"教训";case .artifact:"产物";case .crew:"机组";case .session:"成员";case .channel:"会话";case .message:"消息";case .human:"人类";case .document:"文档";case .knowledge:"资料页面"}}
        var prefix:Character? {switch self {case .work:"W";case .requirement:"#";case .ask:"H";case .table:"T";case .row:"O";case .decision:"D";case .risk:"R";case .lesson:"L";default:nil}}
    }
    struct Target:Codable,Equatable,Identifiable {
        var kind:TargetKind
        var idValue:String?=nil
        var reference:CrewdArtifactWire.Reference?=nil
        var source:CrewdChannelKnowledgeReference?=nil
        enum CodingKeys:String,CodingKey {case kind,idValue="id",reference,source}
        var id:String {if let source {return kind.rawValue+":"+source.identity};return (try? JSONEncoder.sorted.encode(self).base64EncodedString()) ?? "invalid"}
        static func ==(a:Self,b:Self)->Bool {exact(a,b)}
        var valid:Bool {
            switch kind {
            case .human:return idValue==nil && reference==nil && source==nil
            case .artifact:return idValue==nil && reference?.valid==true && source==nil
            case .document,.knowledge:return idValue==nil && reference==nil && source?.valid==true
            default:
                guard let idValue,reference==nil,source==nil,opaque(idValue) else {return false}
                return kind.prefix.map{CrewdManagedDirectoryWire.objectNumber(idValue,prefix:$0) != nil} ?? true
            }
        }
        var label:String {kind.title+" "+(idValue ?? reference?.key ?? source?.reference.id ?? "")}
        var intent:Intent {
            var e:[(String,Intent)]=[("kind",.string(kind.rawValue))]
            if let idValue {e.append(("id",.string(idValue)))}
            if let reference {e.append(("reference",artifactReference(reference)))}
            if let source {e.append(("source",knowledgeReference(source)))}
            return .object(e)
        }
    }
    static func artifactReference(_ r:CrewdArtifactWire.Reference)->Intent {
        var namespace:[(String,Intent)]=[("kind",.string(r.namespace.kind))]
        if let id=r.namespace.id {namespace.append(("id",.string(id)))}
        if let crew=r.namespace.crewId {namespace.append(("crewId",.string(crew)))}
        return object(("namespace",.object(namespace)),("key",.string(r.key)))
    }
    static func artifactInput(_ i:CrewdArtifactWire.Input)->Intent {object(("kind",.string(i.kind.rawValue)),("value",.string(i.value)),("repository",optional(i.repository)))}
    static func knowledgeReference(_ s:CrewdChannelKnowledgeReference)->Intent {
        let c=s.context,r=s.reference
        return object(("context",object(("dbId",.string(c.dbId)),("authorityId",.string(c.authorityId)),("restoreEpoch",.string(c.restoreEpoch)),("crewId",.string(c.crewId)),("namespace",.string(c.namespace)),("generation",.string(c.generation)))),
                      ("reference",object(("namespace",.string(r.namespace)),("kind",.string(r.kind)),("id",.string(r.id)))))
    }
    struct Constraint:Codable,Equatable {
        var kind:String
        var tableId:String?=nil
        static func ==(a:Self,b:Self)->Bool {exact(a,b)}
        var valid:Bool {if kind=="anyReadable" {return tableId==nil};guard (TargetKind(rawValue:kind) != nil && kind != "knowledge") || kind=="knowledgeView" else {return false};return kind=="row" ? (tableId.map{CrewdManagedDirectoryWire.objectNumber($0,prefix:"T") != nil} ?? true):tableId==nil}
        var intent:Intent {kind=="row" ? object(("kind",.string(kind)),("tableId",optional(tableId))):object(("kind",.string(kind)))}
        var title:String {kind=="anyReadable" ? "可读取的对象":kind=="knowledgeView" ? "资料页面":TargetKind(rawValue:kind)?.title ?? "不支持的类型"}
    }
    enum ScalarKind:String,Codable,CaseIterable,Identifiable {case text,integer,real;var id:Self{self};var title:String {switch self {case .text:"文字";case .integer:"整数";case .real:"小数"}}}
    struct FieldKind:Codable,Equatable {
        var kind:String
        var target:Constraint?=nil
        var many:Bool?=nil
        static func ==(a:Self,b:Self)->Bool {exact(a,b)}
        var scalar:ScalarKind? {ScalarKind(rawValue:kind)}
        var link:Bool {kind=="link"}
        var valid:Bool {link ? target?.valid==true && many != nil:scalar != nil && target==nil && many==nil}
        var intent:Intent {link ? object(("kind",.string(kind)),("target",target!.intent),("many",.bool(many!))):object(("kind",.string(kind)))}
        var title:String {link ? ((many==true ? "多个":"单个")+(target?.title ?? "关联对象")):scalar?.title ?? "不支持的类型"}
    }
    struct Field:Codable,Equatable,Identifiable {
        let id:String
        var name:String
        var kind:FieldKind
        var nullable:Bool
        var hidden:Bool
        var valid:Bool {fieldNumber(id) != nil && opaque(name,max:512) && kind.valid}
        static func ==(a:Self,b:Self)->Bool {exact(a,b)}
        var draft:FieldDraft {.init(name:name,kind:kind,nullable:nullable)}
    }
    struct FieldDraft:Codable,Equatable {
        var name:String
        var kind:FieldKind
        var nullable:Bool
        static func ==(a:Self,b:Self)->Bool {exact(a,b)}
        var valid:Bool {label(name,limit:120) && kind.valid}
        var intent:Intent {object(("name",.string(name)),("kind",kind.intent),("nullable",.bool(nullable)))}
    }
    enum Policy:String,Codable,CaseIterable,Identifiable {case authorCaptainHuman,ownerCrewWriters,legacyCrewWriters;var id:Self{self};var title:String {switch self {case .authorCaptainHuman:"作者、现任机长及人类";case .ownerCrewWriters:"所属机组具写权限的成员";case .legacyCrewWriters:"沿用原机组写权限"}}}
    struct Author:Codable,Equatable {let kind:String;var sessionId:String?=nil;var valid:Bool {kind=="session" ? sessionId.map{opaque($0)}==true:["human","legacyUnknown"].contains(kind) && sessionId==nil};var title:String {kind=="human" ? "人类":kind=="legacyUnknown" ? "旧记录，作者未知":"成员 "+(sessionId ?? "")}}
    struct Table:Codable,Identifiable {
        let id:String,crewId:String,schemaRevision:String,title:String
        let purpose:String?
        let titleFieldId:String
        let rowWritePolicy:Policy
        let author:Author
        let fields:[Field]
        let nextFieldNumber:String
        let frozen:Bool,v1Readable:Bool,v1Writable:Bool
        var active:[Field] {fields.filter{!$0.hidden}}
        var valid:Bool {
            guard CrewdManagedDirectoryWire.objectNumber(id,prefix:"T") != nil,opaque(crewId),positive(schemaRevision),opaque(title,max:4096),author.valid,
                  (1...64).contains(fields.count),fields.allSatisfy(\.valid),active.count<=64,unique(fields.map(\.id)),
                  let next=number(nextFieldNumber),fields.allSatisfy({fieldNumber($0.id)!<next}),
                  active.contains(where:{$0.id==titleFieldId && $0.kind.scalar != nil}) else {return false}
            return true
        }
    }
    enum Cell:Codable,Equatable {
        case null,text(String),integer(String),real(Double)
        static func ==(a:Self,b:Self)->Bool {
            switch (a,b) {
            case (.null,.null):return true
            case (.text(let x),.text(let y)),(.integer(let x),.integer(let y)):return x.utf8.elementsEqual(y.utf8)
            case (.real(let x),.real(let y)):return x.bitPattern==y.bitPattern
            default:return false
            }
        }
        enum Keys:String,CodingKey {case type,value}
        init(from decoder:Decoder)throws {
            let c=try decoder.container(keyedBy:Keys.self)
            switch try c.decode(String.self,forKey:.type) {
            case "null":guard !c.contains(.value) else {throw CrewdManagedV2Intent.Failure.invalid};self = .null
            case "text":self = .text(try c.decode(String.self,forKey:.value))
            case "integer":self = .integer(try c.decode(String.self,forKey:.value))
            case "real":self = .real(try c.decode(Double.self,forKey:.value))
            default:throw CrewdManagedV2Intent.Failure.invalid
            }
        }
        func encode(to encoder:Encoder)throws {
            var c=encoder.container(keyedBy:Keys.self)
            switch self {
            case .null:try c.encode("null",forKey:.type)
            case .text(let v):try c.encode("text",forKey:.type);try c.encode(v,forKey:.value)
            case .integer(let v):try c.encode("integer",forKey:.type);try c.encode(v,forKey:.value)
            case .real(let v):guard v.isFinite else {throw CrewdManagedV2Intent.Failure.invalid};try c.encode("real",forKey:.type);try c.encode(v,forKey:.value)
            }
        }
        var display:String {switch self {case .null:"空值";case .text(let s),.integer(let s):s;case .real(let d):String(d)}}
        var intent:Intent {switch self {case .null:object(("type",.string("null")));case .text(let s):object(("type",.string("text")),("value",.string(s)));case .integer(let s):object(("type",.string("integer")),("value",.string(s)));case .real(let d):object(("type",.string("real")),("value",.real(d)))}}
        func accepts(_ field:Field)->Bool {
            switch (self,field.kind.scalar) {
            case (.null,_):return field.nullable && field.kind.scalar != nil
            case (.text(let s),.text):return s.utf8.count<=65_536
            case (.integer(let s),.integer):return Int64(s).map{String($0)==s} ?? false
            case (.real(let d),.real):return d.isFinite
            default:return false
            }
        }
        static func parse(_ text:String,null:Bool,field:Field)throws->Self {
            if null {guard field.nullable,field.kind.scalar != nil else {throw CrewdManagedV2Intent.Failure.invalid};return .null}
            switch field.kind.scalar {
            case .text:return .text(text)
            case .integer:guard let n=Int64(text) else {throw CrewdManagedV2Intent.Failure.invalid};return .integer(String(n))
            case .real:guard let n=Double(text),n.isFinite else {throw CrewdManagedV2Intent.Failure.invalid};return .real(n)
            default:throw CrewdManagedV2Intent.Failure.invalid
            }
        }
    }
    struct ScalarValue:Codable {let fieldId:String;let value:Cell}
    struct VisibleLink:Codable,Identifiable {let edgeId:String;let target:Target;let summary:String;var id:String{edgeId};var valid:Bool{positive(edgeId) && target.valid}}
    struct HiddenSet:Codable {let keepToken:String;var valid:Bool{opaque(keepToken)}}
    struct LinkCell:Codable,Identifiable {let fieldId:String;let visible:[VisibleLink];let hiddenSet:HiddenSet?;let incompleteRequiredLink:Bool;var id:String{fieldId}}
    struct Row:Codable,Identifiable {
        let id:String,crewId:String,tableId:String,schemaRevision:String,revision:String
        let author:Author
        let scalarCells:[ScalarValue],linkCells:[LinkCell]
        let frozen:Bool
        func matches(_ table:Table)->Bool {
            guard table.valid,CrewdManagedDirectoryWire.objectNumber(id,prefix:"O") != nil,crewId.utf8.elementsEqual(table.crewId.utf8),tableId==table.id,schemaRevision==table.schemaRevision,positive(revision),author.valid,
                  unique(scalarCells.map(\.fieldId)),unique(linkCells.map(\.fieldId)) else {return false}
            let byID=Dictionary(uniqueKeysWithValues:table.fields.map{($0.id,$0)})
            for cell in scalarCells {guard let f=byID[cell.fieldId],cell.value.accepts(f) else {return false}}
            for cell in linkCells {
                guard let f=byID[cell.fieldId],f.kind.link,cell.visible.count<=64,cell.visible.allSatisfy(\.valid),unique(cell.visible.map(\.edgeId)),unique(cell.visible.map{$0.target.id}),cell.hiddenSet.map(\.valid) ?? true else {return false}
                if f.kind.many==false && cell.visible.count>1 {return false}
                if cell.incompleteRequiredLink && (f.nullable || !cell.visible.isEmpty || cell.hiddenSet != nil) {return false}
            }
            return true
        }
        func scalar(_ field:String)->Cell {scalarCells.first{$0.fieldId==field}?.value ?? .null}
        func links(_ field:String)->LinkCell? {linkCells.first{$0.fieldId==field}}
    }
}

private extension JSONEncoder {
    static var sorted:JSONEncoder {let encoder=JSONEncoder();encoder.outputFormatting=[.sortedKeys];return encoder}
}

extension CrewdManagedTableV2Wire {
    struct LinkWrite:Codable {
        var kind:String
        var target:Target?=nil
        var input:CrewdArtifactWire.Input?=nil
        var valid:Bool {kind=="reference" ? target?.valid==true && input==nil:kind=="declareArtifact" && target==nil && input != nil}
        var intent:Intent {kind=="reference" ? object(("kind",.string(kind)),("target",target!.intent)):object(("kind",.string(kind)),("input",artifactInput(input!)))}
    }
    struct CellPatch:Codable {
        var kind:String
        var value:Cell?=nil
        var values:[LinkWrite]?=nil
        var keepHiddenSetToken:String?=nil
        var valid:Bool {
            switch kind {
            case "keep":return value==nil && values==nil && keepHiddenSetToken==nil
            case "setScalar":return value != nil && values==nil && keepHiddenSetToken==nil
            case "replaceLinks":return value==nil && values.map{$0.count<=64 && $0.allSatisfy(\.valid)}==true && (keepHiddenSetToken.map{opaque($0)} ?? true)
            default:return false
            }
        }
        var intent:Intent {
            switch kind {
            case "keep":return object(("kind",.string(kind)))
            case "setScalar":return object(("kind",.string(kind)),("value",value!.intent))
            default:return object(("kind",.string(kind)),("values",.array(values!.map(\.intent))),("keepHiddenSetToken",optional(keepHiddenSetToken)))
            }
        }
    }
    struct Patch:Codable {let fieldId:String;var change:CellPatch;var valid:Bool{fieldNumber(fieldId) != nil && change.valid};var intent:Intent{object(("fieldId",.string(fieldId)),("change",change.intent))}}
    struct SchemaChange:Codable {
        var kind:String
        var field:FieldDraft?=nil
        var fieldId:String?=nil
        var name:String?=nil
        var title:String?=nil
        var purpose:String?=nil
        var valid:Bool {
            switch kind {
            case "addField":return field?.valid==true && fieldId==nil && name==nil && title==nil && purpose==nil
            case "renameField":return field==nil && fieldId.flatMap(fieldNumber) != nil && name.map{label($0,limit:120)}==true && title==nil && purpose==nil
            case "renameTable":return field==nil && fieldId==nil && name==nil && title.map{label($0,limit:256)}==true && purpose==nil
            case "setPurpose":return field==nil && fieldId==nil && name==nil && title==nil && purpose.map{label($0,limit:1500)}==true
            case "hideField","restoreField","setTitleField","widenSingleLink":return field==nil && fieldId.flatMap(fieldNumber) != nil && name==nil && title==nil && purpose==nil
            default:return false
            }
        }
        var intent:Intent {
            var entries:[(String,Intent)]=[("kind",.string(kind))]
            if let field {entries.append(("field",field.intent))};if let fieldId {entries.append(("fieldId",.string(fieldId)))}
            if let name {entries.append(("name",.string(name)))};if let title {entries.append(("title",.string(title)))};if let purpose {entries.append(("purpose",.string(purpose)))}
            return .object(entries)
        }
    }
    struct Operation:Codable {
        var kind:String
        var title:String?=nil
        var purpose:String?=nil
        var fields:[FieldDraft]?=nil
        var titleFieldIndex:Int?=nil
        var rowWritePolicy:Policy?=nil
        var tableId:String?=nil
        var rowId:String?=nil
        var expectedSchemaRevision:String?=nil
        var expectedRowRevision:String?=nil
        var change:SchemaChange?=nil
        var patches:[Patch]?=nil
        var valid:Bool {
            switch kind {
            case "createTable":
                return title.map{label($0,limit:256)}==true && purpose.map{label($0,limit:1500)}==true && fields.map{(1...64).contains($0.count) && $0.allSatisfy(\.valid) && unique($0.map(\.name))}==true &&
                    titleFieldIndex.map{fields!.indices.contains($0) && fields![$0].kind.scalar != nil}==true && rowWritePolicy != nil && rowWritePolicy != .legacyCrewWriters &&
                    tableId==nil && rowId==nil && expectedSchemaRevision==nil && expectedRowRevision==nil && change==nil && patches==nil
            case "alterTable":
                return tableId.map{CrewdManagedDirectoryWire.objectNumber($0,prefix:"T") != nil}==true && expectedSchemaRevision.map(positive)==true && change?.valid==true &&
                    title==nil && purpose==nil && fields==nil && titleFieldIndex==nil && rowWritePolicy==nil && rowId==nil && expectedRowRevision==nil && patches==nil
            case "addRow","patchRow":
                guard title==nil,purpose==nil,fields==nil,titleFieldIndex==nil,rowWritePolicy==nil,change==nil,expectedSchemaRevision.map(positive)==true,
                      let patches,patches.count<=64,patches.allSatisfy(\.valid),unique(patches.map(\.fieldId)) else {return false}
                if kind=="addRow" {return tableId.map{CrewdManagedDirectoryWire.objectNumber($0,prefix:"T") != nil}==true && rowId==nil && expectedRowRevision==nil}
                return tableId==nil && rowId.map{CrewdManagedDirectoryWire.objectNumber($0,prefix:"O") != nil}==true && expectedRowRevision.map(positive)==true
            default:return false
            }
        }
        var target:String {tableId ?? rowId ?? kind}
        var intent:Intent {
            var entries:[(String,Intent)]=[("kind",.string(kind))]
            if let title {entries.append(("title",.string(title)))};if let purpose {entries.append(("purpose",.string(purpose)))}
            if let fields {entries.append(("fields",.array(fields.map(\.intent))))};if let titleFieldIndex {entries.append(("titleFieldIndex",.unsigned(UInt64(titleFieldIndex))))}
            if let rowWritePolicy {entries.append(("rowWritePolicy",.string(rowWritePolicy.rawValue)))}
            if let tableId {entries.append(("tableId",.string(tableId)))};if let rowId {entries.append(("rowId",.string(rowId)))}
            if let expectedSchemaRevision {entries.append(("expectedSchemaRevision",.string(expectedSchemaRevision)))};if let expectedRowRevision {entries.append(("expectedRowRevision",.string(expectedRowRevision)))}
            if let change {entries.append(("change",change.intent))};if let patches {entries.append(("patches",.array(patches.map(\.intent))))}
            return .object(entries)
        }
    }
    struct Submit:Codable {let scope:Scope;let crewId:String;let requestId:String;let operation:Operation
        var intent:Intent {object(("method",.string(CrewdManagedTableV2Wire.submit)),("scope",scopeIntent(scope)),("crewId",.string(crewId)),("requestId",.string(requestId)),("operation",operation.intent))}
        func digest()throws->String {guard operation.valid else {throw CrewdManagedV2Intent.Failure.invalid};return try CrewdManagedV2Intent.digest(intent)}
    }
    struct Effect:Codable {
        let kind:String
        let tableId:String
        var rowId:String?=nil
        var schemaRevision:String?=nil
        var fieldIds:[String]?=nil
        var titleFieldId:String?=nil
        var originalSchemaRevision:String?=nil
        var resultSchemaRevision:String?=nil
        var changedFieldIds:[String]?=nil
        var changed:Bool?=nil
        var rowRevision:String?=nil
        var edgeIds:[String]?=nil
        var originalRowRevision:String?=nil
        var resultRowRevision:String?=nil
        var addedEdgeIds:[String]?=nil
        var removedEdgeIds:[String]?=nil
    }
    struct Receipt:Codable {let scope:Scope;let crewId:String;let requestId:String;let recordedAt:String;let operationKind:String;let intentDigest:String;let eventId:String?;let effect:Effect}
    struct Lookup:Decodable {let status:String;let receipt:Receipt?}
    struct TablePage:Decodable {let items:[Table];let nextAfter:String?}
    struct RowPage:Decodable {let table:Table;let items:[Row];let nextAfter:String?}
    struct ResolvedTarget:Decodable {let target:Target;let summary:String;let ownerCrewId:String?;let messageChannelId:String?}
    static let resolveTarget="managedLink.resolveTarget",changed="links.changed"
    struct Source:Codable,Equatable {
        var kind:String
        var id:String?=nil
        var reference:CrewdArtifactWire.Reference?=nil
        var metadataCrewId:String?=nil
        static func ==(a:Self,b:Self)->Bool {exact(a,b)}
        var valid:Bool {
            if kind=="artifact" {return id==nil && reference?.valid==true && metadataCrewId.map{opaque($0)}==true}
            guard let t=TargetKind(rawValue:kind),let prefix=t.prefix,let id,reference==nil,metadataCrewId==nil else {return false}
            return CrewdManagedDirectoryWire.objectNumber(id,prefix:prefix) != nil
        }
        var target:Target? {if kind=="artifact",let reference{return .init(kind:.artifact,reference:reference)};guard let kind=TargetKind(rawValue:kind) else {return nil};return .init(kind:kind,idValue:id)}
        var label:String {kind=="artifact" ? "产物 "+(reference?.key ?? ""):target?.label ?? "关联源"}
        var key:String {(try? JSONEncoder.sorted.encode(self).base64EncodedString()) ?? "invalid"}
        var intent:Intent {
            if kind=="artifact" {return object(("kind",.string(kind)),("reference",artifactReference(reference!)),("metadataCrewId",.string(metadataCrewId!)))}
            return object(("kind",.string(kind)),("id",.string(id!)))
        }
    }
    struct ManualOperation:Codable {
        var kind:String
        let source:Source
        var target:LinkWrite?=nil
        let expectedManualLinkRevision:String
        var expectedEdgeId:String?=nil
        var valid:Bool {
            guard source.valid,number(expectedManualLinkRevision) != nil else {return false}
            if kind=="link" {return target?.valid==true && expectedEdgeId==nil}
            return kind=="unlink" && target==nil && expectedEdgeId.map(positive)==true
        }
        var intent:Intent {
            var e:[(String,Intent)]=[("kind",.string(kind)),("source",source.intent),("expectedManualLinkRevision",.string(expectedManualLinkRevision))]
            if let target {e.append(("target",target.intent))};if let expectedEdgeId {e.append(("expectedEdgeId",.string(expectedEdgeId)))}
            return .object(e)
        }
    }
    struct ManualSubmit:Codable {let scope:Scope;let crewId:String;let requestId:String;let operation:ManualOperation
        var intent:Intent {object(("method",.string(linkSubmit)),("scope",scopeIntent(scope)),("crewId",.string(crewId)),("requestId",.string(requestId)),("operation",operation.intent))}
        func digest()throws->String {guard operation.valid else {throw CrewdManagedV2Intent.Failure.invalid};return try CrewdManagedV2Intent.digest(intent)}
    }
    struct ManualEffect:Codable {let kind:String;let edgeId:String;let added:Bool?}
    struct ManualReceipt:Codable {let scope:Scope;let crewId:String;let requestId:String;let recordedAt:String;let intentDigest:String;let source:Source;let originalManualLinkRevision:String;let resultManualLinkRevision:String;let eventId:String?;let effect:ManualEffect}
    struct ManualLookup:Decodable {let status:String;let receipt:ManualReceipt?}
    struct EdgeOrigin:Codable {let origin:String;var fieldId:String?=nil;var adapter:String?=nil;var readonlyEdgeId:String?=nil
        var valid:Bool {switch origin {case "manualRelates":return fieldId==nil && adapter==nil && readonlyEdgeId==nil;case "column":return fieldId.flatMap(fieldNumber) != nil && adapter==nil && readonlyEdgeId==nil;case "adapter":return fieldId==nil && adapter.map{opaque($0)}==true && readonlyEdgeId.map{opaque($0)}==true;default:return false}}
        var title:String {
            if origin=="manualRelates" {return "手工关联"};if origin=="column" {return "表格关联列 "+(fieldId ?? "")}
            let labels=["work.parent":"上级计划","work.dependsOn":"依赖计划","work.sources":"来源需求","work.blockedOn":"等待处理的事项","work.owner":"实际负责人","ask.item":"问题所属计划","decision.about":"决定涉及对象","decision.supersedes":"被取代的决定","decision.supersededBy":"后续决定","decision.origin":"拍板问题","risk.about":"风险涉及对象","risk.mitigatedBy":"缓解措施","risk.becameLesson":"风险发生所记教训","lesson.learnedFrom":"教训来源","lesson.appliesTo":"适用范围","lesson.risk":"发生的风险","source.message":"原消息出处","artifact.evidence":"产物凭据","artifact.appliesTo":"适用文件","artifact.relates":"相关产物"]
            return labels[adapter ?? ""] ?? "原业务关系"
        }
    }
    struct Edge:Codable,Identifiable {let id:String;let ownerCrewId:String;let source:Source;let target:Target;let summary:String;let origin:EdgeOrigin;let editable:Bool
        var valid:Bool {opaque(id) && opaque(ownerCrewId) && source.valid && target.valid && origin.valid && (origin.origin=="manualRelates" ? positive(id):!editable)}
    }
    struct OutgoingHiddenSet:Decodable {let marker:String}
    struct EdgePage:Decodable {let source:Source?;let manualLinkRevision:String?;let outgoingHiddenSet:OutgoingHiddenSet?;let items:[Edge];let nextAfter:String?}
}

extension CrewdManagedTableV2Wire {
    enum DraftFailure:LocalizedError {
        case changed,required(String),capacity(String),hidden(String),invalid(String)
        var errorDescription:String? {switch self {
        case .changed:"原表定义或记录已变化，请读取最新版本用于对照。"
        case .required(let name):"请为必填关联「"+name+"」选择目标。"
        case .capacity(let name):"关联列「"+name+"」超过当前允许的目标范围。"
        case .hidden(let name):"「"+name+"」含当前不可见原关联，可见项不能净增；原隐藏整组必须保留。"
        case .invalid(let name):"请核对「"+name+"」的字段类型或填写内容。"
        }}
    }
    /// Produces only explicit edits. Missing fields are Keep at the Core boundary.
    static func draftPatches(table:Table,row:Row?,text:[String:String],nulls:Set<String>,links:[String:[LinkWrite]],edited:Set<String>)throws->[Patch] {
        guard table.valid,row.map({$0.matches(table)}) ?? true,
              edited.isSubset(of:Set(table.active.map(\.id))) else {throw DraftFailure.changed}
        var patches:[Patch]=[]
        for field in table.active where row==nil || edited.contains(field.id) {
            let change:CellPatch
            if field.kind.link {
                let values=links[field.id] ?? [],before=row?.links(field.id)
                guard values.allSatisfy(\.valid) else {throw DraftFailure.invalid(field.name)}
                if before?.hiddenSet != nil {
                    guard values.count<=(before?.visible.count ?? 0) else {throw DraftFailure.hidden(field.name)}
                } else if values.count>(field.kind.many==true ? 64:1) {throw DraftFailure.capacity(field.name)}
                guard field.nullable || !values.isEmpty || before?.hiddenSet != nil else {throw DraftFailure.required(field.name)}
                let identities=values.map {value in value.target?.id ?? ((try? JSONEncoder.sorted.encode(value).base64EncodedString()) ?? "invalid")}
                guard unique(identities) else {throw DraftFailure.invalid(field.name)}
                change = .init(kind:"replaceLinks",values:values,keepHiddenSetToken:before?.hiddenSet?.keepToken)
            } else {
                do {let value=try Cell.parse(text[field.id] ?? "",null:nulls.contains(field.id),field:field)
                    guard value.accepts(field) else {throw DraftFailure.invalid(field.name)}
                    change = .init(kind:"setScalar",value:value)
                }catch{throw DraftFailure.invalid(field.name)}
            }
            patches.append(.init(fieldId:field.id,change:change))
        }
        return patches
    }
}
