import Foundation
func o(_ f:[String:CrewdValue])->CrewdValue{.object(f)}
func s(_ t:String)->CrewdValue{.string(t)}
@main struct Tests{
 @MainActor static func main()async throws{
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
}
