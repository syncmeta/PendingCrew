import Foundation
@main struct MobileFlowQA {
 @MainActor static func main() async throws {
  let root=URL(fileURLWithPath:CommandLine.arguments[1]),mode=CommandLine.arguments[2]
  let a="11111111-1111-4111-8111-111111111111",b="22222222-2222-4222-8222-222222222222",bid="bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",fresh="cccccccc-cccc-4ccc-8ccc-cccccccccccc"
  let workspace=CrewdWorkspace(authorityRecordID:UUID(),connect:{client in try await client.connect(path:root.appendingPathComponent("s").path)})
  await workspace.connect();precondition(workspace.client.connected && workspace.crewID==a)
  func signal(_ name:String)throws{try Data().write(to:root.appendingPathComponent(name))}
  func wait(_ name:String)async throws{for _ in 0..<400 {if FileManager.default.fileExists(atPath:root.appendingPathComponent(name).path){return};try await Task.sleep(for:.milliseconds(10))};throw CrewdClientError(message:"control gate not reached "+name)}
  switch mode {
  case "late-directory","same-crew-directory","sync-directory":
   try signal("hold-directory");let old=Task {try await workspace.refreshSessionDirectory(a)};try await wait("held-directory");try signal("fresh-directory")
   if mode=="late-directory" {workspace.selectCrew(b);workspace.selectCrew(a)}
   if mode=="sync-directory" {workspace.scheduleSync();for _ in 0..<400 {if workspace.sessions.first?["id"] == .string(fresh){break};try await Task.sleep(for:.milliseconds(10))}} else {try await workspace.refreshSessionDirectory(a)}
   precondition(workspace.sessions.first?["id"] == .string(fresh));try signal("release-directory")
   var rejected=false;do{try await old.value}catch{rejected=true}
   precondition(rejected && workspace.sessions.first?["id"] == .string(fresh),"late directory must be rejected after a newer actual directory read or A/B/A scope switch")
  case "sync-crews":
   try signal("hold-crews");let old=Task {try await workspace.refreshCrewDirectory()};try await wait("held-crews");try signal("fresh-crews")
   workspace.scheduleSync();for _ in 0..<400 {if workspace.crews.first?["title"] == .string("fresh A"){break};try await Task.sleep(for:.milliseconds(10))}
   precondition(workspace.crews.first?["title"] == .string("fresh A"));try signal("release-crews")
   var rejected=false;do{try await old.value}catch{rejected=true}
   precondition(rejected && workspace.crews.first?["title"] == .string("fresh A"),"older crew directory must not overwrite normal refresh's newer facts")
  case "unknown-message":
   workspace.selectChannel("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee")
   precondition(!workspace.channelID.isEmpty)
   let context=CrewdDrafts.Context.message(crew:a,channel:workspace.channelID,session:workspace.sessionID)
   workspace.messageDrafts.set("original message text before actual EOF",for:context)
   let snapshot=workspace.messageDrafts.snapshot(for:context),params=CrewdValue.object(["channelId":.string(context.channel),"text":.string(snapshot.text),"mentions":.array([])])
   #if QA_TEXT_SUBMISSION
   let accepted=await workspace.performText("message.post",params,snapshot:snapshot)
   #else
   let accepted=await workspace.perform("message.post",params)
   #endif
   precondition(!accepted && !workspace.client.connected,"actual EOF must not manufacture success")
   workspace.messageDrafts.set("newer edited message text",for:context)
   #if QA_TEXT_SUBMISSION
   let retained=workspace.uncertainTextSnapshots
   precondition(workspace.uncertainTextSubmissions.contains{$0.method=="message.post" && $0.params==params && $0.snapshot==snapshot},"unknown submission must retain the complete actual method/params/snapshot")
   #else
   // Observe real old stored snapshot arrays; never insert a made-up fallback.
   func snapshots(_ value:Any,_ depth:Int)->[CrewdDrafts.Snapshot] {
    if let values=value as? [CrewdDrafts.Snapshot] {return values}
    guard depth<10 else{return []}
    return Mirror(reflecting:value).children.flatMap{snapshots($0.value,depth+1)}
   }
   let retained=Mirror(reflecting:workspace).children.filter{($0.label ?? "").contains("uncertain")}.flatMap{snapshots($0.value,0)}
   #endif
   precondition(retained.contains{$0.context==context && $0.text==snapshot.text},"old complete unknown message text was lost after editing the newer draft")
   precondition(workspace.messageDrafts.text(for:context)=="newer edited message text")
  case "late-start":
   let context=CrewdDrafts.Context.initialTask(crew:a,session:workspace.sessionID);workspace.messageDrafts.set("held-start",for:context);let snapshot=workspace.messageDrafts.snapshot(for:context)
   let call=Task {try await workspace.performValue("session.start",.object(["crewId":.string(a),"role":.string("captain"),"executor":.string("codex"),"brief":.string(snapshot.text),"isolation":.bool(true)]))};try await wait("held-start")
   workspace.selectCrew(b);workspace.selectSession(bid);let newer=CrewdDrafts.Context.initialTask(crew:b,session:bid);workspace.messageDrafts.set("new B task",for:newer)
   try signal("release-start");let result=try await call.value;precondition(result["id"].string != nil);workspace.messageDrafts.clearIfUnchanged(snapshot)
   precondition(workspace.crewID==b && workspace.sessionID==bid,"late start for A must not put A's returned session inside the user's selected B crew")
   precondition(workspace.messageDrafts.text(for:newer)=="new B task")
  case "late-create":
   var fields=CrewdCrewCreationDrafts.Fields();fields.title="held-create";fields.directory="/remote/A";fields.parent=a;workspace.crewCreationDrafts.edit(fields);let snapshot=workspace.crewCreationDrafts.begin()!
   let call=Task {try await workspace.performValue("crew.create",.object(["title":.string(snapshot.fields.title),"workingDirectory":.string(snapshot.fields.directory),"parentId":.string(snapshot.fields.parent)]))};try await wait("held-create")
   fields.title="new version";fields.directory="/remote/B";fields.parent=b;workspace.crewCreationDrafts.edit(fields);workspace.selectCrew(b);try signal("release-create")
   _=try await call.value;workspace.crewCreationDrafts.finish(snapshot,accepted:true)
   precondition(workspace.crewCreationDrafts.fields==fields && !workspace.crewCreationDrafts.busy,"old success cannot clear newer full creation input")
   precondition(workspace.crewID==b,"late crew creation success cannot steal user's newer crew selection")
  case "unknown":
   var f=CrewdCrewCreationDrafts.Fields();f.title="bad-receipt";f.directory="/remote/original";f.parent=a;workspace.crewCreationDrafts.edit(f);let s=workspace.crewCreationDrafts.begin()!
   do{_=try await workspace.performValue("crew.create",.object(["title":.string(f.title),"workingDirectory":.string(f.directory),"parentId":.string(f.parent)]));preconditionFailure("missing id accepted")}catch {precondition(CrewdClientError.isUncertainSubmission(error));workspace.crewCreationDrafts.finish(s,accepted:false,uncertain:true)}
   f.title="edited";f.directory="/remote/new";workspace.crewCreationDrafts.edit(f);precondition(workspace.crewCreationDrafts.unknown==[s] && workspace.crewCreationDrafts.fields==f)
   let context=CrewdDrafts.Context.initialTask(crew:a,session:workspace.sessionID);workspace.messageDrafts.set("bad-start",for:context);let start=workspace.messageDrafts.snapshot(for:context)
   do{_=try await workspace.performValue("session.start",.object(["crewId":.string(a),"role":.string("captain"),"executor":.string("codex"),"brief":.string(start.text),"isolation":.bool(true)]));preconditionFailure("missing start id accepted")}catch{precondition(CrewdClientError.isUncertainSubmission(error));workspace.uncertainSessionStarts.append(start)}
   workspace.messageDrafts.set("new task",for:context);precondition(workspace.uncertainSessionStarts.first?.text=="bad-start" && workspace.messageDrafts.text(for:context)=="new task" && !workspace.messageDrafts.clearIfUnchanged(start))
   do{_=try await workspace.performValue("session.start",.object(["crewId":.string(a),"brief":.string("reject")]));preconditionFailure()}catch let error as CrewdClientError {precondition(error.code == -32009 && error.data["refusal"] == .string("session.state_conflict") && !CrewdClientError.isUncertainSubmission(error))}
  default:preconditionFailure("unknown mode")
  }
  workspace.retire();print("PASS: shared mobile operation runtime seam "+mode+"; no GUI/provider/pairing claim")
 }
}
