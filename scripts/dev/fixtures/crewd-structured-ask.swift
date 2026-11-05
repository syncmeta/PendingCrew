import Foundation

@main struct StructuredAskProof {
    typealias Structured = CrewdStructuredAskWire
    typealias Wire = CrewdManagedDirectoryWire
    typealias Store = CrewdManagedDirectoryStore

    @MainActor static func main() async throws {
        let crew="crew-a", ask="H3", decisionID="D9"
        let draft: CrewdValue = .object([
            "question":.string("怎样保存原结果？"), "context":.string("保留能复查的出处。"),
            "options":.array([.object(["label":.string("保留单份")]),.object(["label":.string("分开保存")])]),
            "about":.array([.object(["kind":.string("crew"),"id":.string(crew)])]),
            "default":.object(["optionId":.string("B"),"afterMinutes":.integer(15)]),
            "reversibility":.string("reversible")])
        func record(_ captured:CrewdValue = .null, contents:CrewdValue=draft) -> CrewdValue {
            .object(["crewId":.string(crew),"ask":.string(ask),"draft":contents,"capture":captured])
        }
        func capture(_ source:String,option:String="B",reason:CrewdValue = .null) -> CrewdValue {
            .object(["decisionId":.string(decisionID),"eventSeq":.string("12"),"optionId":.string(option),
                     "source":.string(source),"humanReason":reason])
        }
        func resolve(_ state:String, answer:String? = nil, defaultAfter:Int = 15) throws -> CrewdAskResolutionWire.Review {
            var raw: [String:CrewdValue] = ["crewId":.string(crew),"ask":.string(ask),
                "askedAt":.string("2026-11-05T00:00:00Z"),"question":.string("请选择 A 或 B"),"state":.string(state),
                "default":.object(["text":.string("B"),"afterMinutes":.integer(Int64(defaultAfter))])]
            if let answer { raw["answer"] = .string(answer) }
            return try .init(.object(["ask":.object(raw),"resolutionRevision":state=="open" ?
                .string(String(repeating:"a",count:64)):.null]),crew:crew,id:ask)
        }
        let open=try Structured.Review(record(),crew:crew,ask:ask)
        precondition(open.options.map(\.id)==["A","B"] && open.defaultChoice?.optionId=="B")
        let simple=try open.answer(option:"A",reason:" \n")
        let reasoned=try open.answer(option:"B",reason:"  保留 e\u{301}  \n")
        precondition(simple=="A" && Array(reasoned.utf8)==Array("B\n保留 e\u{301}".utf8))
        for (option,reason) in [("b",""),("C",""),("B",String(repeating:"界",count:501))] {
            do { _ = try open.answer(option:option,reason:reason); preconditionFailure("invalid answer") }
            catch is CrewdClientError {}
        }
        let readOpen=try resolve("open")
        let mismatchedDefault=try resolve("open",defaultAfter:16)
        let human=try Structured.Review(record(capture("human",reason:.string("  保留"))))
        let system=try Structured.Review(record(capture("systemDefault")))
        let humanRaw=try resolve("answered",answer:"B\n  保留")
        let wrongHuman=try resolve("answered",answer:"B")
        let defaultRaw=try resolve("defaulted",answer:"B")
        precondition(open.matches(readOpen) && !open.matches(mismatchedDefault) && !human.matches(readOpen) && human.matches(humanRaw)
                     && !human.matches(wrongHuman) && system.matches(defaultRaw) && !system.matches(humanRaw))
        do { _ = try Structured.Review(record(capture("systemDefault",option:"A"))); preconditionFailure("wrong default capture") }
        catch is CrewdClientError {}
        var calls:[String]=[]
        let work=CrewdWorkStore { method,params in
            precondition(method==Structured.method && params == .object(["crewId":.string(crew),"ask":.string(ask)]))
            calls.append(method)
            return record()
        }
        let hid=CrewdWorkID(ask,kind:.human)!
        let absent=try await work.humanDecisionReview(hid,crew:crew)
        precondition(absent==nil && calls.isEmpty)
        work.configureCapabilities([Structured.capability])
        let typed=try await work.humanDecisionReview(hid,crew:crew)
        precondition(typed?.sameDraft(as:open)==true && calls==[Structured.method])

        let presentation=CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:nil,authorityID:"db",
            databaseID:"db",grantID:nil,grantRevision:nil)
        let origin=Store.Origin(scope:presentation,observation:UUID(),connection:UUID(),crew:crew)
        let authority=Wire.Scope(dbId:"db",restoreEpoch:UUID().uuidString.lowercased())
        let decision=Wire.Decision(id:decisionID,crewId:crew,revision:"1",
            draft:.init(question:open.question,choice:"分开保存",rationale:open.context,alternatives:"A 保留单份",
                reversibility:.reversible,about:[.init(kind:.crew,id:crew)]),
            author:.init(kind:"systemDefault",sessionId:nil),createdAt:"2026-11-05T00:00:00Z",status:.active,
            frozen:false,supersedes:nil,supersededBy:nil,withdrawal:nil)
        var include=false, origins=0
        let managed=Store(origin:{origin},rpc:{ method,params in
            switch method {
            case "managedObject.scope": return try CrewdKnowledgeStore.encode(authority)
            case "managedObject.decisions":
                precondition(params["includeSystemDefaults"] == (include ? .bool(true):.null))
                return .object(["items":try CrewdKnowledgeStore.encode(include ? [decision]:[]),"nextAfter":.null])
            case "managedObject.get":
                precondition(params["includeSystemDefaults"] == .bool(true) && params["id"] == .string(decisionID))
                return .object(["kind":.string("decision"),"decision":try CrewdKnowledgeStore.encode(decision)])
            case Structured.originMethod:
                precondition(params == .object(["id":.string(decisionID)]))
                origins+=1
                return record(capture("systemDefault"))
            default: preconditionFailure("unexpected command")
            }
        },journal:{preconditionFailure("read must never use mutation journal")},decisionsAvailable:{true},
            structuredDecisionsAvailable:{include})
        let scope=try await managed.scope(stillCurrent:{true})
        let old=try await managed.decisions(scope,after:nil,stillCurrent:{true})
        let noOrigin=try await managed.decisionOrigin(scope,decision:decision,stillCurrent:{true})
        precondition(old.items.isEmpty && noOrigin==nil && origins==0)
        include=true
        let page=try await managed.decisions(scope,after:nil,stillCurrent:{true})
        let detail=try await managed.decision(scope,id:decisionID,stillCurrent:{true})
        let source=try await managed.decisionOrigin(scope,decision:detail,stillCurrent:{true})
        precondition(page.items.map(\.id)==[decisionID] && !detail.author.valid &&
                     source?.capture?.source == .systemDefault && source?.ask==ask && origins==1)
        print("PASS structured Ask canonical answer, exact capture, old Core capability and D provenance opt-in")
    }
}
