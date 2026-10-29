import Foundation
func o(_ f:[String:CrewdValue])->CrewdValue{.object(f)}
func s(_ value:String)->CrewdValue{.string(value)}
func replacing(_ value:CrewdValue,_ key:String,_ replacement:CrewdValue)->CrewdValue{guard case .object(var fields)=value else{fatalError()};fields[key]=replacement;return o(fields)}
@main struct RecoveryTests {
 @MainActor static func main()async throws {
  let old=UUID().uuidString.lowercased(),disposition=UUID().uuidString.lowercased(),native=UUID().uuidString.lowercased()
  let target=o(["sessionId":s("session"),"generation":s("1"),"controlRevision":s("1"),"turnId":.null])
  let covered=o(["requestId":s(disposition),"originalRequestIds":.array([s(old)])])
  let inspected=o(["sessionId":s("session"),"originalAgentSessionId":s(native),"target":target,"eligibility":s("evidenceUnavailable"),"evidenceToken":.null,"unresolved":.array([]),"dispositions":.array([covered])])
  let validated=try CrewdClaudeWire.recoveryInspect(inspected,session:"session")
  guard CrewdClaudeWire.recoveryCovered(validated).isEmpty else {print("FAIL: evidenceUnavailable must not release original Unknown");exit(1)}
  print("PASS: unavailable evidence preserves blocker")
  let complete=replacing(inspected,"eligibility",s("noUnresolvedRequests"))
  let checkedComplete=try CrewdClaudeWire.recoveryInspect(complete,session:"session")
  precondition(CrewdClaudeWire.recoveryCovered(checkedComplete) == [old])
  let row=o(["requestId":s(old),"runtimeBindingId":s(UUID().uuidString.lowercased()),"phase":s("unknown"),"historical":.bool(true)])
  var response=replacing(replacing(replacing(inspected,"eligibility",s("eligible")),"unresolved",.array([row])),"evidenceToken",s(String(repeating:"a",count:64)))
  var origin:CrewdClaudeHistoryStore.Origin? = .init(generation:UUID(),connection:UUID(),authority:s("authority"),runtime:s("binding"),crew:"crew")
  var writes:[String]=[],unknown=false,failWrite=false,changeSource=false,failRead=false
  let store=CrewdClaudeHistoryStore(session:"session",crew:"crew",origin:{origin},read:{method,params in
    precondition(method=="session.claude.recoveryInspect" && params==o(["sessionId":s("session")]))
    if failRead{throw CrewdPublicWire.invalid()}
    if changeSource{origin = .init(generation:UUID(),connection:UUID(),authority:s("authority"),runtime:s("binding"),crew:"crew")}
    return response
  },perform:{method,params in
    writes.append(method);precondition(method=="session.claude.recoveryResolve" && params["originalRequestIds"] == .array([s(old)]) && params["target"]==target)
    if failWrite{unknown=true;throw CrewdPublicWire.invalid()}
    response=complete
    return o(["dispositionRecorded":.bool(true),"sessionId":s("session"),"originalRequestIds":.array([s(old)]),"originalOutcome":s("unknown"),"effectsDispatched":.bool(false)])
  },unknown:{true})
  store.recoveryAvailable={true};store.recoveryUnknown={unknown};store.blockedByOriginal={!$0.contains(old)}
  let confirmation=try await store.prepareRecovery();precondition(writes.isEmpty)
  await store.resolveRecovery(confirmation)
  precondition(writes==["session.claude.recoveryResolve"] && store.failure==nil && CrewdClaudeWire.recoveryCovered(store.recovery)==[old])
  print("PASS: explicit disposition writes once, zero resume, original coverage remains")
  response=confirmation.value;failWrite=true
  let uncertain=try await store.prepareRecovery();await store.resolveRecovery(uncertain)
  precondition(writes.count==2 && store.failure != nil && unknown)
  await store.resolveRecovery(uncertain);precondition(writes.count==2)
  do{_ = try await store.prepareRecovery();fatalError("Unknown allowed fresh disposition")}catch{}
  print("PASS: Unknown disposition cannot resubmit or prepare a replacement")
  unknown=false;failWrite=false;response=confirmation.value
  let stale=try await store.prepareRecovery();response=replacing(response,"evidenceToken",s(String(repeating:"b",count:64)))
  await store.resolveRecovery(stale);precondition(writes.count==2)
  changeSource=true
  do{_ = try await store.prepareRecovery();fatalError("source change accepted")}catch{}
  precondition(store.recovery == .null && writes.count==2);changeSource=false
  print("PASS: changed evidence/source reject before dispatch")
  response=complete;try await store.inspectRecovery();failRead=true
  do{try await store.inspectRecovery();fatalError("read failure accepted")}catch{}
  precondition(store.recovery == .null);failRead=false
  store.recoveryAvailable={false};try await store.inspectRecovery();precondition(store.recovery == .null)
  print("PASS: read failure/capability loss discard cached coverage")
  let ids=CrewdValue.array([s(old)]),result=o(["dispositionRecorded":.bool(true),"sessionId":s("session"),"originalRequestIds":ids,"originalOutcome":s("unknown"),"effectsDispatched":.bool(false)])
  for bad in [replacing(result,"effectsDispatched",.bool(true)),replacing(result,"originalOutcome",s("failed")),replacing(result,"originalRequestIds",.array([s(disposition)])),replacing(result,"sessionId",s("other"))]{
    do{try CrewdClaudeWire.recoveryResult(bad,session:"session",originalRequestIDs:ids);fatalError("invalid committed result accepted")}catch{}
  }
  precondition(CrewdPublicOperation(rawValue:"session.claude.recoveryResolve") != nil)
  print("PASS: correlated committed result rejects effects, rewritten outcomes and foreign IDs")
  let request=UUID(),binding=UUID().uuidString.lowercased()
  let params=o(["target":target,"originalAgentSessionId":s(native),"evidenceToken":s(String(repeating:"a",count:64)),"originalRequestIds":ids])
  let envelope=try CrewdPublicCommandEnvelope(request:request,binding:o(["dbId":s("db"),"runtimeBindingId":s(binding)]),operation:.claudeRecoveryResolve,params:params)
  precondition(envelope.params==params && envelope.lookup["requestId"]==s(request.uuidString.lowercased()))
  let receipt=o(["requestId":s(request.uuidString.lowercased()),"dbId":s("db"),"runtimeBindingId":s(binding),"command":envelope.command,"phase":s("committed"),"historical":.bool(false),"payloadSha256":s(String(repeating:"a",count:64)),"createdAt":s("now"),"updatedAt":s("now"),"result":result])
  _ = try CrewdPublicReceipt(receipt,request:request,database:"db",originalRuntimeBinding:binding,originalCommand:envelope.command)
  do{_ = try CrewdPublicReceipt(replacing(receipt,"result",replacing(result,"effectsDispatched",.bool(true))),request:request,database:"db",originalRuntimeBinding:binding,originalCommand:envelope.command);fatalError("receipt accepted dispatched disposition")}catch{}
  do{_ = try CrewdPublicCommandEnvelope(request:request,binding:o(["dbId":s("db"),"runtimeBindingId":s(binding)]),operation:.claudeRecoveryResolve,params:replacing(params,"originalRequestIds",.array([s(old),s(old)])));fatalError("duplicate original accepted")}catch{}
  print("PASS: actual public envelope and receipt enforce original IDs and zero effects")
 }
}
