import Foundation

@main struct ManagedRiskLessonProof {
    typealias Wire = CrewdManagedDirectoryWire
    typealias Store = CrewdManagedDirectoryStore

    @MainActor static func main() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("pcw-rl-proof-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let db = "fixture-db", crew = "crew-a", foreign = "crew-b"
        let ledgerScope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,
            authorityID:db,databaseID:db,grantID:nil,grantRevision:nil)
        let ledger = try CrewdPresentationJournal(root:root,scope:ledgerScope)
        var origin = Store.Origin(scope:ledgerScope,observation:UUID(),connection:UUID(),crew:crew)
        let scope = Wire.Scope(dbId:db,restoreEpoch:UUID().uuidString.lowercased())
        let at = "2026-11-05T01:02:03.456Z", due = "2026-11-12T01:02:03.456Z"
        let later = "2026-11-06T01:02:03.456Z", laterDue = "2026-11-13T01:02:03.456Z"
        let human = Wire.DecisionAuthor(kind:"human",sessionId:nil)
        let session = Wire.DecisionAuthor(kind:"session",sessionId:"s-a")
        let riskDraft = Wire.RiskDraft(summary:"迁移可能影响交接",impact:.high,likelihood:.medium,mitigation:"分批",
            about:[.init(kind:.crew,id:crew)],mitigatedBy:[.init(kind:.decision,id:"D2")])
        var changedDraft = riskDraft
        changedDraft.mitigation = "分批并复查"
        let draft = Wire.LessonDraft(happened:"切换时有一次中断。",lesson:"保留 e\u{301} 记录",detection:"看原日志",
            learnedFrom:[.init(kind:.work,id:"W4")],evidence:["fixture:proof"],appliesTo:[.init(kind:.crew,id:crew)])
        var derived = draft
        derived.learnedFrom.append(.init(kind:.risk,id:"R3"))

        func risk(_ revision:String="1", draft:Wire.RiskDraft=riskDraft, owner:Wire.DecisionAuthor=human,
                  updated:String=at, review:String=due, status:Wire.RiskStatus = .open, reason:String?=nil,
                  link:String?=nil, frozen:Bool=false) -> Wire.Risk {
            .init(id:"R3",crewId:crew,revision:revision,draft:draft,owner:owner,author:human,createdAt:at,
                  updatedAt:updated,nextReviewAt:review,status:status,statusReason:reason,becameLessonId:link,frozen:frozen)
        }
        func lesson(_ id:String, owner:String=crew, revision:String="1", draft:Wire.LessonDraft=draft,
                    created:String=later, checked:String=later, status:Wire.LessonStatus = .active,
                    reason:String?=nil, source:String?=nil) -> Wire.Lesson {
            .init(id:id,crewId:owner,revision:revision,draft:draft,author:human,createdAt:created,lastCheckedAt:checked,
                  status:status,staleReason:reason,riskId:source,frozen:false)
        }
        func value<T:Encodable>(_ x:T) throws -> CrewdValue { try CrewdKnowledgeStore.encode(x) }
        func riskObject(_ item:Wire.Risk) throws -> CrewdValue { .object(["kind":.string("risk"),"risk":try value(item)]) }
        func lessonObject(_ item:Wire.Lesson) throws -> CrewdValue { .object(["kind":.string("lesson"),"lesson":try value(item)]) }
        func receipt(_ original:CrewdValue, object:CrewdValue, recordedAt:String, occurrence:CrewdValue = .null) -> CrewdValue {
            .object(["scope":original["scope"],"crewId":original["crewId"],"requestId":original["requestId"],
                     "recordedAt":.string(recordedAt),"object":object,"occurrence":occurrence])
        }
        let effect: CrewdValue = .object(["riskId":.string("R3"),"originalRiskRevision":.string("1"),
            "resultRiskRevision":.string("2"),"lessonId":.string("L5"),"lessonRevision":.string("1")])
        let linked = lesson("L5",draft:derived,source:"R3")
        let ownLesson = lesson("L8")
        let otherLesson = lesson("L10",owner:foreign,created:at,checked:at)
        let browse = Store.BrowseScope(origin:origin,authority:scope)
        func validate(_ mutation:Store.RiskLessonMutation, kind:String, object:CrewdValue,
                      time:String=later, occurrence:CrewdValue = .null) throws {
            let request = UUID().uuidString.lowercased()
            let (change,witness) = try mutation.prepare(scope:browse,request:request)
            let params = try value(Wire.Submit(scope:scope,crewId:witness.crewId,requestId:request,change:change))
            let result = try CrewdKnowledgeStore.decode(receipt(params,object:object,recordedAt:time,occurrence:occurrence),Wire.Receipt.self)
            precondition(params["change"]["kind"] == .string(kind) && result.object.valid)
            try Store.RiskLessonMutation.verify(result,original:witness)
        }
        try validate(.recordRisk(riskDraft,human),kind:"recordRisk",object:riskObject(risk()),time:at)
        try validate(.updateRisk(risk(),changedDraft),kind:"updateRisk",
                     object:riskObject(risk("2",draft:changedDraft,updated:later,review:laterDue)))
        try validate(.assignRisk(risk(),session),kind:"assignRisk",object:riskObject(risk("2",owner:session,updated:later)))
        try validate(.reviewRisk(risk()),kind:"reviewRisk",object:riskObject(risk("2",updated:later,review:laterDue)))
        try validate(.setRiskStatus(risk(owner:session),.mitigated,"已分批"),kind:"setRiskStatus",
                     object:riskObject(risk("2",owner:session,updated:later,status:.mitigated,reason:"已分批")))
        let occurred = risk("2",updated:later,status:.occurred,link:"L5")
        try validate(.setRiskStatus(occurred,.closed,"已跟进"),kind:"setRiskStatus",
                     object:riskObject(risk("3",updated:later,status:.closed,reason:"已跟进",link:"L5")))
        try validate(.occurrence(risk(),draft),kind:"recordRiskOccurrence",object:lessonObject(linked),occurrence:effect)
        try validate(.recordLesson(draft),kind:"recordLesson",object:lessonObject(ownLesson))
        try validate(.confirmLesson(otherLesson),kind:"confirmLesson",
                     object:lessonObject(lesson("L10",owner:foreign,revision:"2",created:at)))
        try validate(.staleLesson(ownLesson,"不再适用"),kind:"staleLesson",
                     object:lessonObject(lesson("L8",revision:"2",status:.stale,reason:"不再适用")))
        precondition(risk().due(at:Wire.managedTime(due)!) && !occurred.due(at:Wire.managedTime(due)!))

        var available = false
        var submissions: [String] = []
        var recoveredIDs: [String] = []
        var lost: CrewdValue?
        var malformed = true
        var wireCalls: [String] = []
        func make(_ journal:CrewdPresentationJournal) -> Store {
            Store(origin:{origin},rpc:{method,params in
                wireCalls.append(method)
                switch method {
                case "managedObject.scope": return try value(scope)
                case "managedObject.lessons":
                    return .object(["items":try value([otherLesson,linked]),"nextAfter":.null])
                case "managedObject.submit":
                    let kind = params["change"]["kind"].string!
                    submissions.append(kind)
                    if kind == "recordRiskOccurrence" {
                        precondition(params["crewId"] == .string(crew) && params["change"]["draft"] == .null &&
                            params["change"]["lessonDraft"]["learnedFrom"].array.count == 1)
                        lost = try receipt(params,object:lessonObject(linked),recordedAt:later,occurrence:effect)
                    } else {
                        precondition(kind == "confirmLesson" && params["crewId"] == .string(foreign))
                        lost = try receipt(params,object:lessonObject(lesson("L10",owner:foreign,revision:"2",created:at)),recordedAt:later)
                    }
                    throw CrewdClientError(message:"reply lost")
                case "managedObject.request.get":
                    recoveredIDs.append(params["requestId"].string!)
                    precondition(params["crewId"] == lost!["crewId"])
                    if malformed {
                        guard case .object(var fields) = lost! else { preconditionFailure("fixture receipt") }
                        fields["occurrence"] = .object(["riskId":.string("R3"),"originalRiskRevision":.string("1"),
                            "resultRiskRevision":.string("3"),"lessonId":.string("L5"),"lessonRevision":.string("1")])
                        return .object(["status":.string("known"),"receipt":.object(fields)])
                    }
                    return .object(["status":.string("known"),"receipt":lost!])
                default: preconditionFailure("unexpected wire route")
                }
            },journal:{journal},riskLessonsAvailable:{available})
        }
        let frontend = make(ledger)
        let first = try await frontend.scope(stillCurrent:{true})
        do {
            _ = try await frontend.riskLessons(first,lessons:true,after:nil,stillCurrent:{true})
            preconditionFailure("old capability issued L query")
        } catch Wire.Problem.unavailable {}
        precondition(wireCalls == ["managedObject.scope"])
        available = true
        let page = try await frontend.riskLessons(first,lessons:true,after:nil,stillCurrent:{true})
        precondition(page.items.map(\.id) == ["L10","L5"])
        for mutation in [Store.RiskLessonMutation.occurrence(risk(),draft),.confirmLesson(otherLesson)] {
            do {
                _ = try await frontend.submit(.riskLesson(mutation),scope:first,stillCurrent:{true})
                preconditionFailure("lost reply was observed")
            } catch let error as CrewdClientError { precondition(error.message == "reply lost") }
            let pending = try frontend.pending(first)
            precondition(pending.count == 1 && pending[0].record.params["draft"] == .null)
            if pending[0].witness.kind == "confirmLesson" {
                precondition(pending[0].record.crew == foreign && pending[0].witness.presentationCrew == crew)
            } else {
                do {
                    _ = try await frontend.lookup(pending[0],scope:first,stillCurrent:{true})
                    preconditionFailure("mismatched atomic effect acknowledged")
                } catch Wire.Problem.invalid {}
                malformed = false
            }
            let found = try await frontend.lookup(pending[0],scope:first,stillCurrent:{true})
            precondition(found?.receipt.requestId == pending[0].witness.requestId)
        }
        origin = .init(scope:ledgerScope,observation:UUID(),connection:UUID(),crew:crew)
        let reopened = make(try CrewdPresentationJournal(root:root,scope:ledgerScope))
        let reopenedScope = try await reopened.scope(stillCurrent:{true})
        let remaining = try reopened.pending(reopenedScope)
        precondition(remaining.isEmpty && submissions == ["recordRiskOccurrence","confirmLesson"] && recoveredIDs.count == 3)
        print("PASS R/L lifecycle, due date, atomic occurrence, foreign confirmation scope and no replay")
    }
}
