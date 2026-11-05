import Foundation

@main struct ManagedDirectoryProof {
    typealias Wire = CrewdManagedDirectoryWire
    typealias Store = CrewdManagedDirectoryStore

    @MainActor static func main() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("pcw-managed-journal-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let db = "fixture-db", crew = "crew-a", foreign = "crew-b"
        let ledgerScope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,
            authorityID:db,databaseID:db,grantID:nil,grantRevision:nil)
        let journal = try CrewdPresentationJournal(root:root,scope:ledgerScope)
        var origin: Store.Origin? = .init(scope:ledgerScope,observation:UUID(),connection:UUID(),crew:crew)
        let oldScope = Wire.Scope(dbId:db,restoreEpoch:UUID().uuidString.lowercased())
        var activeScope = oldScope
        let fields: [Wire.Field] = [.init(name:"名称",fieldType:.text,nullable:false),
                                    .init(name:"数量",fieldType:.integer,nullable:false),
                                    .init(name:"比例",fieldType:.real,nullable:true)]
        let table = Wire.Table(id:"T2",crewId:crew,title:"组织联系人",fields:fields,frozen:false)
        let other = Wire.Table(id:"T10",crewId:foreign,title:"可阅读",fields:fields,frozen:false)
        let original: [Wire.Cell] = [.text("原始"),.integer("9223372036854775807"),.null]
        let changed: [Wire.Cell] = [.text("e\u{301}"),.integer("-9223372036854775808"),.real(1.25)]
        var storedReceipt: CrewdValue?
        var sent: [String] = []
        var requested: [CrewdValue] = []
        enum ReadMode { case absent, wrongPayload, dismiss, known, historical }
        var mode = ReadMode.absent
        var current = true

        func value<T:Encodable>(_ x:T) throws -> CrewdValue { try CrewdKnowledgeStore.encode(x) }
        func row(_ values:[Wire.Cell], revision:String, id:String="O12", table:Wire.Table) -> Wire.Row {
            .init(id:id,crewId:table.crewId,tableId:table.id,revision:revision,values:values,frozen:table.frozen)
        }
        func reply(_ object:CrewdValue,params:CrewdValue) -> CrewdValue {
            .object(["scope":params["scope"],"crewId":params["crewId"],"requestId":params["requestId"],
                     "recordedAt":.string("2026-11-05T00:00:00Z"),"object":object])
        }
        func make(_ activeJournal:CrewdPresentationJournal) -> Store {
            Store(origin:{origin},rpc:{method,params in
                switch method {
                case "managedObject.scope": return try value(activeScope)
                case "managedObject.tables":
                    return .object(["items":try value([table,other]),"nextAfter":.null])
                case "managedObject.rows":
                    let selected = params["tableId"].string == other.id ? other:table
                    return .object(["table":try value(selected),
                                    "items":try value([row(original,revision:"1",table:selected)]),"nextAfter":.null])
                case "managedObject.submit":
                    precondition(params["crewId"] == .string(crew))
                    let change=params["change"], kind=change["kind"].string!
                    sent.append(kind)
                    if kind == "createTable" {
                        precondition(change["title"] == .string(table.title))
                        return reply(.object(["kind":.string("table"),"table":try value(table)]),params:params)
                    }
                    if kind == "addRow" {
                        precondition(change["tableId"] == .string(table.id))
                        return reply(.object(["kind":.string("row"),
                                             "row":try value(row(original,revision:"1",table:table))]),params:params)
                    }
                    precondition(change["rowId"] == .string("O12"))
                    precondition(change["expectedRevision"] == .string("1"))
                    storedReceipt=reply(.object(["kind":.string("row"),
                                                 "row":try value(row(changed,revision:"2",table:table))]),params:params)
                    throw CrewdClientError(message:"reply deliberately lost")
                case "managedObject.request.get":
                    requested.append(params)
                    switch mode {
                    case .absent: return .object(["status":.string("notFound")])
                    case .wrongPayload:
                        return .object(["status":.string("known"),"receipt":reply(.object([
                            "kind":.string("row"),"row":try value(row([.text("é"),changed[1],changed[2]],
                                                                     revision:"2",table:table))
                        ]),params:params)])
                    case .dismiss:
                        current=false
                        return .object(["status":.string("known"),"receipt":storedReceipt!])
                    case .known: return .object(["status":.string("known"),"receipt":storedReceipt!])
                    case .historical: return .object(["status":.string("historical"),"receipt":storedReceipt!])
                    }
                default: preconditionFailure("unexpected method " + method)
                }
            },journal:{activeJournal})
        }
        let store=make(journal)
        let scope=try await store.scope(stillCurrent:{true})
        let tables=try await store.tables(scope,after:nil,stillCurrent:{true})
        precondition(tables.items.map(\.id) == ["T2","T10"])
        precondition(!Wire.validPage(["T10","T2"],after:nil,next:nil,prefix:"T"))
        precondition(!Wire.validPage(["T2"],after:"T01",next:nil,prefix:"T"))
        let foreignRows=try await store.rows(scope,table:other.id,after:nil,stillCurrent:{true})
        precondition(foreignRows.table.crewId == foreign && foreignRows.items.count == 1)
        do {
            _ = try await store.submit(.add(other,original),scope:scope,stillCurrent:{true})
            preconditionFailure("foreign write reached transport")
        } catch Wire.Problem.invalid {}
        let created=try await store.submit(.create(table.title,fields),scope:scope,stillCurrent:{true})
        precondition(created.receipt.object.table?.id == table.id)
        let added=try await store.submit(.add(table,original),scope:scope,stillCurrent:{true})
        precondition(added.receipt.object.row?.id == "O12")
        let originalRow=try await store.rows(scope,table:table.id,after:nil,stillCurrent:{true}).items[0]
        do {
            _ = try await store.submit(.edit(table,originalRow,changed),scope:scope,stillCurrent:{true})
            preconditionFailure("lost reply was acknowledged")
        } catch let error as CrewdClientError { precondition(error.message == "reply deliberately lost") }

        let saved=try store.pending(scope)
        precondition(saved.count == 1 && saved[0].witness.requestId == storedReceipt!["requestId"].string)
        precondition(saved[0].record.params["values"] == .null && saved[0].record.params["change"] == .null)
        let duplicateJournal=try CrewdPresentationJournal(root:root,scope:ledgerScope)
        let recovered=make(duplicateJournal)
        origin = .init(scope:ledgerScope,observation:UUID(),connection:UUID(),crew:crew)
        let reconnected=try await recovered.scope(stillCurrent:{true})
        let pending=try recovered.pending(reconnected)[0]
        do {
            _ = try await recovered.submit(.edit(table,originalRow,changed),scope:reconnected,stillCurrent:{true})
            preconditionFailure("unknown target was resent")
        } catch let error as CrewdClientError { precondition(error.kind == .notSubmitted) }
        let absent=try await recovered.lookup(pending,scope:reconnected,stillCurrent:{true})
        precondition(absent == nil)
        mode = .wrongPayload
        do {
            _ = try await recovered.lookup(pending,scope:reconnected,stillCurrent:{true})
            preconditionFailure("canonically equivalent but byte-distinct value acknowledged")
        } catch Wire.Problem.invalid {}
        mode = .dismiss
        do {
            _ = try await recovered.lookup(pending,scope:reconnected,stillCurrent:{current})
            preconditionFailure("dismissed lookup acknowledged")
        } catch Wire.Problem.stale {}
        let stillPending=try recovered.pending(reconnected)
        precondition(stillPending.count == 1)
        current=true

        activeScope = .init(dbId:db,restoreEpoch:UUID().uuidString.lowercased())
        let restored=try await recovered.scope(stillCurrent:{true})
        mode = .known
        do {
            _ = try await recovered.lookup(pending,scope:restored,stillCurrent:{true})
            preconditionFailure("historical receipt was presented as current")
        } catch Wire.Problem.invalid {}
        mode = .historical
        let result=try await recovered.lookup(pending,scope:restored,stillCurrent:{true})
        precondition(result?.historical == true && result?.receipt.requestId == pending.witness.requestId)
        let remains=try recovered.pending(restored)
        precondition(remains.isEmpty)
        precondition(sent == ["createTable","addRow","editRow"] && requested.count == 5)
        precondition(requested.allSatisfy { $0["requestId"] == .string(pending.witness.requestId)
            && $0["scope"]["restoreEpoch"] == .string(oldScope.restoreEpoch) })
        print("PASS managed directory foreign read, exact receipts, one edit, no replay, reconnect and restore fence")
    }
}
