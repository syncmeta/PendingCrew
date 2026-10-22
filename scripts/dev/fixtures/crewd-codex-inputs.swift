import Foundation
func o(_ f:[String:CrewdValue])->CrewdValue{.object(f)}
func s(_ t:String)->CrewdValue{.string(t)}
@MainActor func deny(_ action:()async throws->Void)async{do{try await action();fatalError("expected reject")}catch{}}
@MainActor final class Rig{
 let root=FileManager.default.temporaryDirectory.appendingPathComponent("pcw-codex-input-"+UUID().uuidString)
 let journal:CrewdPresentationJournal
 let scope:CrewdPresentationJournal.Scope
 var origin:CrewdCodexInputStore.Origin?
 var calls:[(String,CrewdValue)]=[]
 var route:((String,CrewdValue)async throws->CrewdValue)?
 let context=CrewdCodexInputStore.Context(crew:"crew",session:nil)
 lazy var store=makeStore()
 init()throws{scope = .init(namespace:UUID(),localDirectory:nil,authorityID:UUID().uuidString,databaseID:"opaque",grantID:UUID().uuidString,grantRevision:"1");journal=try .init(root:root,scope:scope);origin = .init(scope:scope,generation:UUID(),connection:UUID(),crew:"crew")}
 func makeStore()->CrewdCodexInputStore{.init(origin:{self.origin},rpc:{m,p in self.calls.append((m,p));if let route=self.route{return try await route(m,p)};return try self.normal(m,p)},journal:{self.journal})}
 func result(_ p:CrewdValue)->CrewdValue{let count=p["bytesBase64"].string.flatMap{Data(base64Encoded:$0)?.count} ?? p["bytes"].int ?? 0;return o(["operation":s("prepared"),"input":o(["inputId":s(UUID().uuidString),"sha256":p["sha256"],"kind":p["kind"],"bytes":.integer(Int64(count))])])}
 func normal(_ m:String,_ p:CrewdValue)throws->CrewdValue{switch m{case "session.codex.inputPrepare":return result(p);case "session.codex.inputDiscard":return o(["operation":s("discarded"),"inputId":p["input"]["inputId"]]);default:throw CrewdPublicWire.invalid()}}
 func prepare(_ text:String="original",store:CrewdCodexInputStore?=nil)async throws{try await (store ?? self.store).prepare(data:Data(text.utf8),filename:"notes.txt",mime:"text/plain",kind:"text",context:context)}
 var target:CrewdValue{o(["sessionId":s("session"),"generation":s("1"),"controlRevision":s("1"),"turnId":s("turn")])}
}
@main struct Tests{
 @MainActor static func main()async throws{
  let mode=CommandLine.arguments.last ?? "",r=try Rig()
  switch mode{
  case "choiceScope":
   let choice=CrewdCodexInputChoice(scope:r.scope,skills:[o(["name":s("native"),"path":s("/actual/skill")])])
   _ = try r.store.saveChoice(choice,context:r.context,lane:"start",expected:nil,scope:r.scope)
   let otherScope=CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:nil,authorityID:r.scope.authorityID,databaseID:r.scope.databaseID,grantID:UUID().uuidString,grantRevision:"1")
   let other=try CrewdPresentationJournal(root:r.root,scope:otherScope)
   let changed=CrewdCodexInputStore(origin:{.init(scope:otherScope,generation:UUID(),connection:UUID(),crew:"crew")},rpc:{_,_ in fatalError("no RPC")},journal:{other})
   await deny{_ = try changed.saveChoice(choice,context:r.context,lane:"start",expected:nil,scope:r.scope)}
   await deny{_ = try choice.value(store:changed,context:r.context)}
   precondition(try! changed.restoreChoice(r.context,lane:"start").0.skills.isEmpty)
  case "catalog":
   let raw=o(["items":.array([o(["id":s("profile"),"allowed":.bool(true),"description":.null])]),"nextCursor":.null])
   precondition(try! CrewdCodexInputWire.profiles(raw).count==1)
   await deny{_ = try CrewdCodexInputWire.profiles(o(["result":raw]))}
   await deny{_ = try CrewdCodexInputWire.profiles(o(["mcpServers":.array([]),"skills":.array([])]))}
   let skills=o(["mcpServers":.array([]),"skills":.array([o(["cwd":s("/actual"),"skills":.array([o(["name":s("native"),"path":s("/actual/skill"),"enabled":.bool(true)])]),"errors":.array([])])])])
   precondition(try! CrewdCodexInputWire.tools(skills).count==1)
   await deny{_ = try CrewdCodexInputWire.tools(raw)}
  case "originalBytes":
   try await r.prepare("original media bytes")
   let files=(FileManager.default.enumerator(at:r.root,includingPropertiesForKeys:nil)?.allObjects as? [URL]) ?? []
   precondition(files.contains{(try? Data(contentsOf:$0))==Data("original media bytes".utf8)},"accepted upload lost original local bytes after reopen")
  case "offline":
   let ticket=try r.store.prepareLocal(data:Data("offline raw".utf8),filename:"notes.txt",mime:"text/plain",kind:"text",context:r.context)
   r.origin=nil
   let archives=try CrewdPresentationJournal.codexArchives(root:r.root)
   precondition(archives.count==1 && archives[0].records.count==1)
   precondition(try! archives[0].journal.codexInputOriginal(ticket.record)==Data("offline raw".utf8))
   precondition(r.calls.isEmpty)
  case "durable":
   let original=try r.store.prepareLocal(data:Data("durable original".utf8),filename:"notes.txt",mime:"text/plain",kind:"text",context:r.context)
   precondition(r.calls.isEmpty)
   let root=FileManager.default.temporaryDirectory.appendingPathComponent("unused")
   _ = root
   let row=try r.store.entries(r.context).first!;precondition(try! r.store.original(row)==Data("durable original".utf8))
   let choice=CrewdCodexInputChoice(scope:r.scope,ids:[row.id],skills:[o(["name":s("native"),"path":s("/actual/skill")])])
   let revision=try r.store.saveChoice(choice,context:r.context,lane:"start",expected:nil,scope:r.scope)
   let reopen=r.makeStore(),saved=try reopen.restoreChoice(r.context,lane:"start");precondition(saved.0==choice && saved.1==revision && r.calls.isEmpty)
   await deny{_ = try reopen.saveChoice(.init(scope:r.scope),context:r.context,lane:"start",expected:nil,scope:r.scope)}
   let next=try reopen.saveChoice(.init(scope:r.scope),context:r.context,lane:"start",expected:revision,scope:r.scope);precondition(next != revision)
   precondition(try! reopen.restoreChoice(r.context,lane:"queue").0 == .init(scope:r.scope))
   try await r.store.sendPrepared(original)
   precondition(r.calls.count==1)
  case "prepare":
   try await r.prepare();let row=try r.store.entries(r.context).first!;precondition(row.state=="prepared");let values=try r.store.selected([row.id],context:r.context);precondition(values.count==1)
   precondition(row.record.params["bytesBase64"] == .null && row.record.params["filename"]==s("notes.txt"))
   let reopen=r.makeStore();let reopened=try reopen.entries(r.context);precondition(reopened.first?.reference==row.reference)
  case "discard":
   try await r.prepare();let row=try r.store.entries(r.context).first!;try await r.store.discard(row,context:r.context);precondition(try! r.store.entries(r.context).first!.state=="discarded");await deny{_ = try r.store.selected([row.id],context:r.context)}
  case "unknown","malformed","wrongHash","lookup","notFound":
   r.route={m,p in if m=="session.codex.inputPrepare"{if mode=="malformed"{return o(["state":s("known"),"result":r.result(p)])};if mode=="wrongHash"{return o(["operation":s("prepared"),"input":o(["inputId":s(UUID().uuidString),"sha256":s(String(repeating:"a",count:64)),"kind":s("text"),"bytes":.integer(8)])])};throw CrewdClientError(message:"lost")};return try r.normal(m,p)}
   await deny{try await r.prepare()};let pending=try r.store.pending(r.context);precondition(pending.count==1)
   let reopened=r.makeStore(),count=r.calls.count;await deny{try await r.prepare(store:reopened)};precondition(r.calls.count==count)
   r.route={m,p in if m=="session.codex.inputLookup"{precondition(p["requestId"]==pending[0].params["requestId"]);if mode=="notFound"{return o(["state":s("notFound")])};return o(["state":s("known"),"result":r.result(pending[0].params)])};return try r.normal(m,p)}
   try await reopened.lookup(pending[0]);let remaining=try reopened.pending(r.context);precondition(remaining.count==(mode=="notFound" ? 1:0))
   // Independent new file is not held behind an unrelated original request.
   try await r.prepare("independent",store:reopened)
  case "used":
   try await r.prepare();let row=try r.store.entries(r.context).first!,selection=o(["inputs":.array([row.reference!])]),command=o(["operation":s("session.codex.queue"),"params":o(["inputs":selection])])
   try r.journal.validateUnusedCodexInputs(command);_ = try r.journal.prepare(method:"command.submit",params:o(["command":command]),crew:"crew")
   await deny{try r.journal.validateUnusedCodexInputs(command)};await deny{_ = try r.store.selected([row.id],context:r.context)}
   let reopen=r.makeStore();await deny{_ = try reopen.selected([row.id],context:r.context)}
  case "switch":
   r.route={m,p in let result=try r.normal(m,p);r.origin = .init(scope:r.scope,generation:UUID(),connection:UUID(),crew:"other");return result}
   await deny{try await r.prepare()};await deny{_ = try r.store.selected([],context:r.context)}
   let records=try r.journal.codexInputRecords();precondition(try! r.journal.codexInputResult(records[0]) != nil)
  case "selection":
   let ref=o(["inputId":s(UUID().uuidString),"sha256":s(String(repeating:"a",count:64)),"kind":s("text"),"bytes":.integer(3)])
   _ = try CrewdCodexInputWire.selection(o(["inputs":.array([ref])]))
   await deny{_ = try CrewdCodexInputWire.selection(o(["inputs":.array([ref,ref])]))}
   await deny{_ = try CrewdCodexInputWire.selection(o(["skills":.array([o(["name":s("skill"),"path":s("relative")])])]))}
   await deny{_ = try CrewdCodexInputWire.selection(o([:]))}
  case "permissions":
   _ = try CrewdCodexWire.settings(o(["permissions":s("profile")]))
   await deny{_ = try CrewdCodexWire.settings(o(["permissions":s("profile"),"sandbox":o(["type":s("dangerFullAccess")])]))}
  case "history":
   let binding=o(["dbId":s("opaque"),"runtimeBindingId":s(UUID().uuidString)])
   for op in [CrewdPublicOperation.codexArchive,.codexUnarchive,.codexDelete,.codexRevert]{var fields:[String:CrewdValue]=["target":r.target];if op == .codexDelete{fields["confirmNativeHistoryDelete"] = .bool(true)};if op == .codexRevert{fields["beforeTurnId"] = s("turn")};let command=try CrewdPublicCommandEnvelope(request:UUID(),binding:binding,operation:op,params:o(fields));try CrewdCodexWire.validateResult(o(["operation":s(op.rawValue),"sessionId":s("session"),"providerAccepted":.bool(true),"application":s("metadata")]),command:command.command)}
   await deny{_ = try CrewdPublicCommandEnvelope(request:UUID(),binding:binding,operation:.codexDelete,params:o(["target":r.target,"confirmNativeHistoryDelete":.bool(false)]))}
  case "queue","start":
   let ref=o(["inputId":s(UUID().uuidString),"sha256":s(String(repeating:"a",count:64)),"kind":s("text"),"bytes":.integer(3)]),selection=o(["inputs":.array([ref])]),binding=o(["dbId":s("opaque"),"runtimeBindingId":s(UUID().uuidString)])
   let params=mode=="queue" ? o(["target":r.target,"text":s("verbatim Human"),"inputs":selection]):o(["crewId":s("crew"),"role":s("worker"),"executor":s("codex"),"brief":s("verbatim Human"),"isolation":.bool(false),"codexInputs":selection])
   let command=try CrewdPublicCommandEnvelope(request:UUID(),binding:binding,operation:mode=="queue" ? .codexQueue:.sessionStart,params:params);precondition(command.params[mode=="queue" ? "inputs":"codexInputs"]==selection)
  default:fatalError("mode")
  }
  print("PASS "+mode)
 }
}
