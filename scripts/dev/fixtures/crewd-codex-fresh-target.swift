import Foundation
#if PENDINGCREW_ACTUAL_APP_FIXTURE
import Combine
@testable import PendingCrew
#if PENDINGCREW_CURRENT_CODEX_STORE
typealias CrewdValue = PendingCrew.CrewdValue
typealias CrewdCodexInspection = PendingCrew.CrewdCodexInspection
typealias CrewdPublicOperation = PendingCrew.CrewdPublicOperation
typealias CrewdCommandTarget = PendingCrew.CrewdCommandTarget
typealias CrewdClientError = PendingCrew.CrewdClientError
typealias CrewdPublicWire = PendingCrew.CrewdPublicWire
#endif
#endif
#if PENDINGCREW_ACTUAL_APP_FIXTURE
@MainActor private final class HeldDraftIdentity {
 var continuation:CheckedContinuation<String,Error>?
 func read()async throws->String {try await withCheckedThrowingContinuation{continuation=$0}}
 func release(){continuation!.resume(returning:"fixture-only-device");continuation=nil}
}
#endif
func o(_ f:[String:CrewdValue])->CrewdValue{.object(f)}
func s(_ t:String)->CrewdValue{.string(t)}
@main struct Tests{
 @MainActor static func main()async throws{
 #if PENDINGCREW_ACTUAL_APP_FIXTURE
 if CommandLine.arguments.last!.hasPrefix("initial-") { try await initialRead(CommandLine.arguments.last!); return }
 #endif
 let mode=CommandLine.arguments.last!,target=o(["sessionId":s("session"),"generation":s("1"),"controlRevision":s("1"),"turnId":s("turn")])
 var epoch="original",calls=0,reads=0
 let inspection=o(["sessionId":s("session"),"target":target,"running":.bool(true),"working":.bool(false),"historical":.bool(false),"queuedInputs":.integer(0),"title":o(["source":s("human"),"status":s("synced")]),"capabilities":.array(["steer","archive","queue"].map{o(["id":s($0),"availability":s("available"),"protocol":s("stable")])})])
 let store=CrewdCodexSessionStore(sessionID:"session",read:{method,params in reads+=1;if method=="session.codex.inspect"{return inspection};precondition(method=="session.controlTarget");var result:[String:CrewdValue]=["sessionId":s("session"),"generation":s("1"),"controlRevision":s("1"),"turnId":s("turn")];if mode=="stale"{result["controlRevision"] = s("2")};if mode=="turn"{result["turnId"] = s("next")};if mode=="scope"{epoch="different"};return o(result)},submit:{operation,params in calls+=1;precondition(params["target"]==target);if mode=="unknown"{throw CrewdClientError(message:"unknown")};return o(["confirmed":.bool(true)])},scope:{epoch},available:{true})
 await store.refresh();let exact=try CrewdCommandTarget(target)
 let operation:CrewdPublicOperation=mode=="turn" ? .codexSteer:.codexArchive
 let succeeded=await store.submit(operation,fields:[:],target:exact)
 precondition(succeeded == (mode=="success"))
 precondition(calls == (["success","unknown"].contains(mode) ? 1:0))
 precondition(reads>=2)
 print("PASS "+mode)
 }

 #if PENDINGCREW_ACTUAL_APP_FIXTURE
 @MainActor static func initialRead(_ mode:String) async throws {
  func require(_ value:Bool,_ message:String) {
   if !value { fputs("FAIL: "+message+"\n",stderr);exit(5) }
  }
  var scope:String?="first", publications=0, writes=0
  var pending:[CheckedContinuation<CrewdValue,Error>]=[]
  let value=o(["sessionId":s("session"),"threadId":s("original-thread"),"target":o(["sessionId":s("session"),"generation":s("1"),"controlRevision":s("1"),"turnId":.null]),"running":.bool(false),"working":.bool(false),"historical":.bool(true),"queuedInputs":.integer(0),"title":o(["source":s("task"),"status":s("synced")]),"capabilities":.array([])])
  let store=CrewdCodexSessionStore(sessionID:"session",read:{method,params in
   require(method=="session.codex.inspect" && params["sessionId"] == s("session"),"initial read used another method or target")
   return try await withCheckedThrowingContinuation { pending.append($0) }
  },submit:{_,_ in writes+=1;return .null},scope:{scope},available:{true})
  let observation=store.objectWillChange.sink {publications+=1}
  defer { observation.cancel();store.retire() }
  let identity=HeldDraftIdentity()
  var restoring:Task<CrewComposerDraftStore.Draft?,Never>?
  var personal:CrewPersonalDraftService?
  let draftKey=AppleDraftKey(authorityID:"fixture",databaseID:"fixture",crewID:"crew",destination:.originalSession("session"),kind:"sessionMessage")
  if mode=="initial-draft" {
   let root=URL(fileURLWithPath:CommandLine.arguments[1],isDirectory:true).appendingPathComponent("personal-input")
   let service=CrewPersonalDraftService.owned(root:root,deviceIdentity:{try await identity.read()},configuration:nil)
   personal=service;restoring=Task{await service.restoreOwn(draftKey)}
   while identity.continuation==nil {await Task.yield()}
   service.checkpoint(.init(text:"new unsent draft"),key:draftKey)
  }
  let first=Task {await store.refresh()}
  while pending.isEmpty {await Task.yield()}
  require(!store.busy,"read-only inspection must not acquire the command busy gate")
  if mode=="initial-draft" {
   pending[0].resume(returning:value);await first.value
   require(store.inspection != nil && identity.continuation != nil,"held draft identity blocked independent stopped-session inspection")
   identity.release()
   _ = await restoring!.value
   require(personal?.localDraft(draftKey)?.text=="new unsent draft","late draft read replaced a newer unsent edit")
  } else if mode=="initial-visible" {
   require(publications>0,"pending first inspection publishes no loading state")
   await store.refresh()
   require(pending.count==1,"same-scope initial reads were not deduplicated")
   pending[0].resume(returning:value);await first.value
   require(store.inspection != nil && store.error==nil,"stopped inspection was not accepted")
  } else if mode=="initial-refresh" {
   pending[0].resume(returning:value);await first.value
   let refresh=Task{await store.refresh()}
   while pending.count<2 {await Task.yield()}
   require(store.inspection != nil && !store.busy,"read-only refresh hid a valid target or blocked command controls")
   pending[1].resume(returning:value);await refresh.value
  } else if mode=="initial-cancel" {
   first.cancel();pending[0].resume(returning:value);await first.value
   require(store.inspection==nil && store.error==nil,"cancelled initial read published a late result/error")
  } else if mode=="initial-scope" {
   scope="second"
   let second=Task {await store.refresh()}
   for _ in 0..<10000 {if pending.count==2{break};await Task.yield()}
   require(pending.count==2,"a stale inflight read prevents the new ready scope from loading")
   var fresh=value
   if case .object(var fields)=fresh {fields["threadId"]=s("fresh-thread");fresh = .object(fields)}
   pending[1].resume(returning:fresh);await second.value
   pending[0].resume(returning:value);await first.value
   require(store.inspection?.value["threadId"]==s("fresh-thread"),"late old scope replaced the new inspection")
  } else if mode=="initial-error" {
   pending[0].resume(throwing:CrewdClientError(message:"bounded inspect failure",kind:.notSubmitted));await first.value
   require(store.inspection==nil && store.error?.contains("bounded inspect failure")==true,"initial read failure is not visible")
  } else {preconditionFailure("unknown initial mode")}
  require(writes==0,"initial loading executed a command")
  print("PASS "+mode+"; read-only, command busy unchanged, writes=0")
 }
 #endif
}
