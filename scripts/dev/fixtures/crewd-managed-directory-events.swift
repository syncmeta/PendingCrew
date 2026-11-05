import Foundation

@main struct ManagedDirectoryEventsProof {
    @MainActor static func main() async throws {
        let base = CrewdClient.eventTopics(capabilities:["managedObjects.v1","data"],includePersonalDraft:false)
        precondition(base.contains("data") && !base.contains("onto") && !base.contains("personalDraft"))
        let capable: Set<String> = ["managedObjects.v1","managedObjects.events.v1","data"]
        let initial = CrewdClient.eventTopics(capabilities:capable,includePersonalDraft:false)
        let replacement = CrewdClient.eventTopics(capabilities:capable,includePersonalDraft:true)
        precondition(initial.filter { $0 == "onto" }.count == 1 && !initial.contains("personalDraft"))
        precondition(Set(replacement) == Set(initial).union(["personalDraft"]) && replacement.count == initial.count+1)
        precondition(!CrewdClient.eventTopics(capabilities:["managedObjects.v1"],includePersonalDraft:true).contains("onto"))

        typealias Store = CrewdManagedDirectoryStore
        let firstConnection = UUID(), secondConnection = UUID()
        let authority = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:nil,
            authorityID:"authority",databaseID:"db",grantID:"grant",grantRevision:"1")
        var origin: Store.Origin? = .init(scope:authority,observation:UUID(),connection:firstConnection,crew:"selected")
        let store = Store(origin:{origin},rpc:{_,_ in preconditionFailure("event must never perform an RPC or mutation")},
                          journal:{preconditionFailure("event must not read or mutate journal")})
        func event(_ ids:[CrewdValue],crew:String="foreign") -> CrewdValue {
            .object(["crewId":.string(crew),"objectIds":.array(ids)])
        }
        let oldScope = Store.BrowseScope(origin:origin!,authority:.init(dbId:"db",restoreEpoch:"restore"))
        store.handleEvent("onto.changed",event([.string("O2")]),connection:UUID())
        precondition(store.invalidation == nil)
        store.handleEvent("onto.changed",event([.string("T1"),.string("O2")]),connection:firstConnection)
        let original = store.invalidation!
        precondition(store.owns(original,scope:oldScope), "foreign committed event was dropped")
        for bad in [event([]),event([.string("T01")]),event([.string("R0")]),
                    event([.string("D9223372036854775808")]),event([.string("W1")]),
                    event([.string("D2"),.string("D2")]),event([.string("R1"),.string("L2"),.string("D3")]),
                    event([.number(4)]),event([.string("L3")],crew:""),
                    .object(["crewId":.string("foreign"),"objectIds":.array([.string("D3")]),"content":.string("unexpected")])] {
            store.handleEvent("onto.changed",bad,connection:firstConnection)
            precondition(store.invalidation == original)
        }
        origin = nil
        store.handleEvent("events.lagged",.null,connection:firstConnection)
        precondition(store.invalidation == original && !store.owns(original,scope:oldScope))
        origin = .init(scope:authority,observation:UUID(),connection:secondConnection,crew:"selected")
        let newScope = Store.BrowseScope(origin:origin!,authority:oldScope.authority)
        store.handleEvent("onto.changed",event([.string("R3"),.string("L4")]),connection:firstConnection)
        precondition(store.invalidation == original && !store.owns(original,scope:newScope))
        store.handleEvent("events.lagged",.null,connection:secondConnection)
        let lag = store.invalidation!
        precondition(lag != original && store.owns(lag,scope:newScope))
        store.handleEvent("crew.purged",.object(["crewId":.string("foreign")]),connection:secondConnection)
        let purge = store.invalidation!
        precondition(purge != lag && store.owns(purge,scope:newScope))
        origin = .init(scope:authority,observation:UUID(),connection:secondConnection,crew:"other-selection")
        precondition(!store.owns(purge))
        print("PASS managed directory event topic compatibility, closed payload and stale scope fences")
    }
}
