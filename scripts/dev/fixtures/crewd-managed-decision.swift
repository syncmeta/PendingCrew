import Foundation

@main struct ManagedDecisionProof {
    typealias Wire = CrewdManagedDirectoryWire
    typealias Store = CrewdManagedDirectoryStore

    @MainActor static func main() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("pcw-decision-proof-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at:root) }
        let db = "decision-db", home = "crew-a", foreign = "crew-b"
        let presentation = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,
            authorityID:db,databaseID:db,grantID:nil,grantRevision:nil)
        let ledger = try CrewdPresentationJournal(root:root,scope:presentation)
        var origin = Store.Origin(scope:presentation,observation:UUID(),connection:UUID(),crew:home)
        let authority = Wire.Scope(dbId:db,restoreEpoch:UUID().uuidString.lowercased())
        let human = Wire.DecisionAuthor(kind:"human",sessionId:nil)
        let agent = Wire.DecisionAuthor(kind:"session",sessionId:"captain-a")
        let timestamp = "2026-11-05T00:00:00Z"
        let original = Wire.DecisionDraft(question:"如何整理会议记录？",choice:"先保留原始记录",
            rationale:"保留可以复查的出处。",alternatives:nil,reversibility:.reversible,
            about:[.init(kind:.crew,id:home),.init(kind:.work,id:"W3")])
        var replacement = original
        replacement.choice = "分开保存 e\u{301}"
        replacement.rationale = "把工作记录和出处分开。"
        let foreignDraft = Wire.DecisionDraft(question:"其他机组怎么办？",choice:"另定",
            rationale:"该决定由其他机组负责。",about:[.init(kind:.crew,id:foreign)])

        func item(_ id:String, _ draft:Wire.DecisionDraft, _ author:Wire.DecisionAuthor,
                  owner:String=home, revision:String="1", status:Wire.DecisionStatus = .active,
                  previous:String?=nil, next:String?=nil,
                  withdrawal:Wire.DecisionWithdrawal?=nil, frozen:Bool=false) -> Wire.Decision {
            .init(id:id,crewId:owner,revision:revision,draft:draft,author:author,createdAt:timestamp,
                  status:status,frozen:frozen,supersedes:previous,supersededBy:next,withdrawal:withdrawal)
        }

        var decision = item("D2",original,human)
        let other = item("D10",foreignDraft,agent,owner:foreign)
        let frozen = item("D8",original,human,frozen:true)
        var successor: Wire.Decision?
        var methods: [String] = []
        var submitted: [(String,String)] = []
        var lookups: [String] = []
        var savedReceipt: CrewdValue?
        enum Reply { case known, wrongLink, dismiss, wrongWithdraw }
        var replyMode = Reply.known
        var available = false
        var current = true
        var refuseInput = true
        let reason = "根据最新资料撤回。"

        func raw<T:Encodable>(_ value:T) throws -> CrewdValue { try CrewdKnowledgeStore.encode(value) }
        func object(_ decision:Wire.Decision) throws -> CrewdValue {
            .object(["kind":.string("decision"),"decision":try raw(decision)])
        }
        func receipt(_ decision:Wire.Decision,_ request:CrewdValue) throws -> CrewdValue {
            .object(["scope":request["scope"],"crewId":request["crewId"],"requestId":request["requestId"],
                     "recordedAt":.string(timestamp),"object":try object(decision)])
        }

        func store(_ journal:CrewdPresentationJournal) -> Store {
            Store(origin:{origin},rpc:{method,params in
                methods.append(method)
                switch method {
                case "managedObject.scope": return try raw(authority)
                case "managedObject.decisions":
                    let page = params["after"].string == "D10" ? [decision] : [other]
                    return .object(["items":try raw(page),"nextAfter":params["after"] == .null ? .string("D10") : .null])
                case "managedObject.get":
                    switch params["id"].string {
                    case "D2": return try object(decision)
                    case "D8": return try object(frozen)
                    case "D10": return try object(other)
                    case "D11": return try object(successor!)
                    default: preconditionFailure("wrong detail identity")
                    }
                case "managedObject.submit":
                    precondition(params["crewId"] == .string(home))
                    let kind=params["change"]["kind"].string!, request=params["requestId"].string!
                    submitted.append((request,kind))
                    switch kind {
                    case "recordDecision":
                        if refuseInput {
                            throw CrewdClientError(message:"invalid target",code:-32004,
                                data:.object(["refusal":.string("resource.not_found")]))
                        }
                        precondition(params["change"]["draft"]["question"] == .string(original.question))
                        return try receipt(decision,params)
                    case "replaceDecision":
                        precondition(params["change"]["decisionId"] == .string("D2")
                                     && params["change"]["expectedRevision"] == .string("1"))
                        decision = item("D2",original,human,revision:"2",status:.superseded,next:"D11")
                        let new = item("D11",replacement,human,previous:"D2")
                        successor = new
                        savedReceipt = try receipt(new,params)
                        throw CrewdClientError(message:"reply lost")
                    case "withdrawDecision":
                        precondition(params["change"]["decisionId"] == .string("D11")
                                     && params["change"]["expectedRevision"] == .string("1"))
                        let end = item("D11",replacement,human,revision:"2",status:.withdrawn,previous:"D2",
                                       withdrawal:.init(author:human,reason:reason,at:timestamp))
                        successor = end
                        savedReceipt = try receipt(end,params)
                        if replyMode == .wrongWithdraw {
                            var changed = replacement
                            changed.choice = "分开保存 é"
                            return try receipt(item("D11",changed,human,revision:"2",status:.withdrawn,previous:"D2",
                                withdrawal:.init(author:human,reason:reason,at:timestamp)),params)
                        }
                        return try receipt(end,params)
                    default: preconditionFailure("wrong mutation")
                    }
                case "managedObject.request.get":
                    lookups.append(params["requestId"].string!)
                    precondition(params["scope"]["restoreEpoch"].string == authority.restoreEpoch)
                    if replyMode == .dismiss { current = false }
                    let result = replyMode == .wrongLink
                        ? try receipt(item("D11",replacement,human,previous:"D3"),params)
                        : savedReceipt!
                    return .object(["status":.string("known"),"receipt":result])
                default: preconditionFailure("unexpected method " + method)
                }
            },journal:{journal},decisionsAvailable:{available})
        }

        let frontend = store(ledger)
        let firstScope = try await frontend.scope(stillCurrent:{true})
        do {
            _ = try await frontend.decisions(firstScope,after:nil,stillCurrent:{true})
            preconditionFailure("old capability performed D read")
        } catch Wire.Problem.unavailable {}
        precondition(methods == ["managedObject.scope"])
        available = true
        let page = try await frontend.decisions(firstScope,after:nil,stillCurrent:{true})
        let older = try await frontend.decisions(firstScope,after:page.nextAfter,stillCurrent:{true})
        precondition(page.items.map(\.id) == ["D10"] && older.items.map(\.id) == ["D2"])
        precondition(!Wire.validPage(["D2","D10"],after:nil,next:nil,prefix:"D",newestFirst:true))
        let readOther = try await frontend.decision(firstScope,id:"D10",stillCurrent:{true})
        precondition(readOther.author == agent && !readOther.canChange(crew:home))
        let readFrozen = try await frontend.decision(firstScope,id:"D8",stillCurrent:{true})
        for bad in [readOther,readFrozen] {
            do {
                _ = try await frontend.submit(.withdrawDecision(bad,reason),scope:firstScope,stillCurrent:{true})
                preconditionFailure("foreign or frozen D mutated")
            } catch Wire.Problem.invalid {}
        }
        precondition(submitted.isEmpty)
        do {
            _ = try await frontend.submit(.recordDecision(original),scope:firstScope,stillCurrent:{true})
            preconditionFailure("typed input refusal lost")
        } catch let error as CrewdClientError { precondition(Store.decisionInputRefused(error)) }
        let pendingAfterRefusal = try frontend.pending(firstScope)
        precondition(pendingAfterRefusal.isEmpty)
        refuseInput = false
        let recorded = try await frontend.submit(.recordDecision(original),scope:firstScope,stillCurrent:{true})
        precondition(recorded.receipt.object.id == "D2")
        do {
            _ = try await frontend.submit(.replaceDecision(decision,replacement),scope:firstScope,stillCurrent:{true})
            preconditionFailure("lost replacement acknowledged")
        } catch let error as CrewdClientError { precondition(error.message == "reply lost") }

        let reconnected = store(try CrewdPresentationJournal(root:root,scope:presentation))
        origin = .init(scope:presentation,observation:UUID(),connection:UUID(),crew:home)
        let scope = try await reconnected.scope(stillCurrent:{true})
        let pending = try reconnected.pending(scope)[0]
        precondition(pending.record.params["draft"] == .null && pending.witness.target == "D2")
        replyMode = .wrongLink
        do {
            _ = try await reconnected.lookup(pending,scope:scope,stillCurrent:{true})
            preconditionFailure("wrong predecessor receipt acknowledged")
        } catch Wire.Problem.invalid {}
        replyMode = .dismiss
        do {
            _ = try await reconnected.lookup(pending,scope:scope,stillCurrent:{current})
            preconditionFailure("dismissed recovery acknowledged")
        } catch Wire.Problem.stale {}
        let stillPending = try reconnected.pending(scope)
        precondition(stillPending.count == 1)
        replyMode = .known
        let recovered = try await reconnected.lookup(pending,scope:scope,stillCurrent:{true})
        precondition(recovered?.receipt.object.decision?.supersedes == "D2" &&
                     recovered?.receipt.requestId == pending.witness.requestId)
        let beforeWithdraw = try await reconnected.decision(scope,id:"D11",stillCurrent:{true})
        replyMode = .wrongWithdraw
        do {
            _ = try await reconnected.submit(.withdrawDecision(beforeWithdraw,reason),scope:scope,stillCurrent:{true})
            preconditionFailure("different old draft bytes acknowledged as a withdrawal")
        } catch Wire.Problem.invalid {}
        let withdrawal = try reconnected.pending(scope)[0]
        replyMode = .known
        let withdrawn = try await reconnected.lookup(withdrawal,scope:scope,stillCurrent:{true})
        precondition(withdrawn?.receipt.object.decision?.withdrawal?.author == human)
        let history = try await reconnected.decision(scope,id:"D2",stillCurrent:{true})
        let end = try await reconnected.decision(scope,id:"D11",stillCurrent:{true})
        precondition(history.status == .superseded && history.supersededBy == "D11" && history.draft == original &&
                     history.author == human && end.status == .withdrawn && end.supersedes == "D2")
        let remaining = try reconnected.pending(scope)
        precondition(remaining.isEmpty)
        precondition(submitted.map { $0.1 } == ["recordDecision","recordDecision","replaceDecision","withdrawDecision"] &&
                     Set(submitted.map { $0.0 }).count == 4 && lookups == Array(repeating:pending.witness.requestId,count:3) + [withdrawal.witness.requestId])
        print("PASS managed D capability, numeric history, record/replace/withdraw, exact original lookup and no replay")
    }
}
