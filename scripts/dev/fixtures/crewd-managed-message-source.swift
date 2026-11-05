import Foundation

@main struct ManagedMessageSourceProof {
    typealias Wire=CrewdManagedDirectoryWire
    typealias Message=CrewdManagedMessageWire
    typealias Store=CrewdManagedDirectoryStore
    @MainActor static func main() async throws {
        if CommandLine.arguments.count == 4,CommandLine.arguments[1] == "--prepare" {
            let root=URL(fileURLWithPath:CommandLine.arguments[2]),namespace=UUID(uuidString:CommandLine.arguments[3])!
            let scope=CrewdPresentationJournal.Scope(namespace:namespace,localDirectory:root.path,authorityID:"race-db",databaseID:"race-db",grantID:nil,grantRevision:nil)
            let ledger=try CrewdPresentationJournal(root:root,scope:scope)
            do {
                _=try ledger.prepareMessageSource(.object(["original":.object(["crewId":.string("crew")]),"sourceMessageId":.string("message"),"key":.string("recordDecision:")]),crew:"crew")
                exit(0)
            } catch {exit(23)}
        }
        let root=FileManager.default.temporaryDirectory.appendingPathComponent("pcw-source-journal-"+UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let db="fixture-db",crew="crew-a",source="actual-source"
        let ledgerScope=CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,authorityID:db,databaseID:db,grantID:nil,grantRevision:nil)
        var ledger=try CrewdPresentationJournal(root:root,scope:ledgerScope)
        let origin=Store.Origin(scope:ledgerScope,observation:UUID(),connection:UUID(),crew:crew)
        var authority=Wire.Scope(dbId:db,restoreEpoch:UUID().uuidString.lowercased())
        let fields:[Wire.Field]=[.init(name:"正文",fieldType:.text,nullable:false)]
        let table=Wire.Table(id:"T1",crewId:crew,title:"已确认事项",fields:fields,frozen:false)
        let values:[Wire.Cell]=[.text("e\u{301}")]
        var current=true,enabled=true,lost=true,created=true
        var submits=0,lookups=0
        var receipt:CrewdValue = .null
        var status="known"
        var dismissDuringLookup=false,corrupt=false
        func value<T:Encodable>(_ input:T)throws->CrewdValue {try CrewdKnowledgeStore.encode(input)}
        func make()->Store {
            Store(origin:{origin},rpc:{method,p in
                if method=="managedObject.scope" { return try value(authority) }
                if method==Message.record {
                    submits+=1
                    let row=Wire.Row(id:"O2",crewId:created ? crew:"other-crew",tableId:"T1",revision:created ? "1":"4",values:created ? values:[.text("earlier object")],frozen:false)
                    receipt = .object(["scope":p["scope"],"crewId":p["crewId"],"requestId":p["requestId"],"recordedAt":.string("2026-11-05T00:00:00Z"),"sourceMessageId":p["sourceMessageId"],"created":.bool(created),"object":.object(["kind":.string("row"),"row":try value(row)])])
                    if lost { throw CrewdClientError(message:"lost reply") };return receipt
                }
                if method==Message.lookup {
                    lookups+=1
                    precondition(p["requestId"]==receipt["requestId"])
                    if dismissDuringLookup {current=false}
                    var result=receipt
                    if corrupt {
                        guard case .object(var top)=result,case .object(var object)=top["object"],case .object(var row)=object["row"] else { fatalError() }
                        row["values"]=try value([Wire.Cell.text("é")]);object["row"] = .object(row);top["object"] = .object(object);result = .object(top)
                    }
                    if status=="notFound" {return .object(["status":.string(status)])}
                    return .object(["status":.string(status),"receipt":result])
                }
                preconditionFailure("Unexpected / write fallback: "+method)
            },journal:{ledger},messageSourcesAvailable:{enabled})
        }
        var store=make()
        var scope=try await store.scope(stillCurrent:{current})
        do { _=try await store.recordFromMessage(.add(table,values),message:source,scope:scope,stillCurrent:{current});fatalError("lost reply accepted") } catch {}
        precondition(submits==1)
        let original=try store.messagePending(scope,message:source).only()
        let request=original.witness.original.requestId
        // Actual journal reopen with a new Store: original UUID only, never fallback submit.
        ledger=try CrewdPresentationJournal(root:root,scope:ledgerScope);store=make()
        do { _=try await store.recordFromMessage(.add(table,values),message:source,scope:scope,stillCurrent:{current});fatalError("unresolved request replayed") } catch {}
        precondition(submits==1)
        let reopened=try store.messagePending(scope,message:source).only()
        precondition(reopened.witness.original.requestId==request)
        corrupt=true
        do {_=try await store.lookupMessage(reopened,scope:scope,stillCurrent:{current});fatalError("canonically equivalent different bytes accepted")}catch{}
        try require(ledger.unknownRecords().count==1)
        corrupt=false;dismissDuringLookup=true
        do {_=try await store.lookupMessage(reopened,scope:scope,stillCurrent:{current});fatalError("dismissed lookup acknowledged")}catch{}
        try require(ledger.unknownRecords().count==1)
        current=true;dismissDuringLookup=false;status="notFound"
        let missing=try await store.lookupMessage(reopened,scope:scope,stillCurrent:{current});precondition(missing == nil)
        try require(ledger.unknownRecords().count==1)
        status="known"
        let recovered=try await store.lookupMessage(reopened,scope:scope,stillCurrent:{current})
        precondition(recovered?.receipt.created==true && recovered?.receipt.requestId==request)
        try require(ledger.unknownRecords().isEmpty)
        // A later explicit attempt gets the existing foreign object and preserves a different draft.
        lost=false;created=false
        let secondValues:[Wire.Cell]=[.text("new draft that must remain local")]
        let reused=try await store.recordFromMessage(.add(table,secondValues),message:source,scope:scope,stillCurrent:{current})
        precondition(!reused.receipt.created && reused.receipt.object.row?.crewId=="other-crew")
        let history=try store.messagePending(scope,message:source)
        precondition(history.count==2)
        guard let saved=history.first(where:{$0.witness.original.requestId==reused.receipt.requestId}),case .add(_,let savedValues)=try store.messageDraft(saved) else {fatalError("draft lost")}
        precondition(savedValues==secondValues)
        // Protected restore lookup must be historical and uses its immutable old scope.
        authority = .init(dbId:db,restoreEpoch:UUID().uuidString.lowercased());status="historical"
        scope=try await store.scope(stillCurrent:{current})
        let historical=try await store.lookupMessage(saved,scope:scope,stillCurrent:{current})
        precondition(historical?.historical==true)
        enabled=false
        let before=submits
        do {_=try await store.recordFromMessage(.add(table,values),message:source,scope:scope,stillCurrent:{current});fatalError("old Core wrote")}catch{}
        precondition(submits==before)
        // Privacy projection is closed, including restricted fields and exact available source crew.
        try require(Message.Source(.object(["status":.string("restricted")])) == .restricted)
        for bad:CrewdValue in [
            .object(["status":.string("restricted"),"messageId":.string(source)]),
            .object(["status":.string("deleted"),"excerpt":.string("retained"),"channelId":.string("hidden")]),
            .object(["status":.string("available"),"messageId":.string(source),"channelId":.string("channel"),"excerpt":.string("text")])
        ] { do {_=try Message.Source(bad);fatalError("private/invalid source admitted")}catch{} }
        try require(Message.Source(.object(["status":.string("available"),"crewId":.string(crew),"messageId":.string(source),"channelId":.string("channel"),"excerpt":.string("actual")])) == .available(crew:crew,message:source,channel:"channel",excerpt:"actual"))
        let raceRoot=root.appendingPathComponent("race"),raceNamespace=UUID()
        // Initialize the owner-private namespace once; race only actual scan+prepare.
        _=try CrewdPresentationJournal(root:raceRoot,scope:.init(namespace:raceNamespace,localDirectory:raceRoot.path,authorityID:"race-db",databaseID:"race-db",grantID:nil,grantRevision:nil))
        let children=(0..<2).map { _ in Process() }
        for child in children {
            child.executableURL=URL(fileURLWithPath:CommandLine.arguments[0])
            child.arguments=["--prepare",raceRoot.path,raceNamespace.uuidString]
            try child.run()
        }
        for child in children {child.waitUntilExit()}
        precondition(children.map(\.terminationStatus).sorted()==[0,23])
        print("PASS two actual processes produce exactly one prepare winner")
        print("PASS exact source namespace; original UUID after journal reopen; no replay; byte-exact proof; dismissed lookup retains; unknown retained; reused foreign object preserves full draft; historical scope; legacy capability; source privacy")
        print("submits=\(submits) lookups=\(lookups)")
    }
}
private extension Array {func only()throws->Element {precondition(count==1);return self[0]}}

private func require(_ value:@autoclosure () throws -> Bool) rethrows {let result=try value();precondition(result)}
