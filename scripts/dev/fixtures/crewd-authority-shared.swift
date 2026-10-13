import Foundation
@main struct AuthorityQA {
 @MainActor static func main()async throws {
  let aPath=CommandLine.arguments[1],bPath=CommandLine.arguments[2];var oldCalls=0,newCalls=0
  let a=CrewdWorkspace(authorityRecordID:UUID(),connect:{client in oldCalls+=1;try await client.connect(path:aPath)})
  let b=CrewdWorkspace(authorityRecordID:UUID(),connect:{client in newCalls+=1;try await client.connect(path:bPath)})
  precondition(a.authorityRecordID != b.authorityRecordID && a.dataDirectory == nil && b.dataDirectory == nil)
  await a.connect();await b.connect();precondition(a.client.connected && b.client.connected)
  a.workStore.selectCrew("crew");b.workStore.selectCrew("crew");try await a.workStore.refresh();try await b.workStore.refresh()
  let w=CrewdWorkID("W1",kind:.plan)!,r=CrewdWorkID("#1",kind:.requirement)!,h=CrewdWorkID("H1",kind:.human)!
  precondition(a.workStore.plans[w]?.value["title"] == .string("A plan") && b.workStore.plans[w]?.value["title"] == .string("B plan"))
  precondition(a.workStore.requirements[r]?["text"] == .string("A requirement") && b.workStore.requirements[r]?["text"] == .string("B requirement"))
  precondition(a.workStore.humanTasks[h]?["ask"]["question"] == .string("A question") && b.workStore.humanTasks[h]?["ask"]["question"] == .string("B question"))
  a.requirementDrafts.set("crew",text:"A draft");b.requirementDrafts.set("crew",text:"B draft")
  let aa=a.requirementDrafts.begin("crew")!,bb=b.requirementDrafts.begin("crew")!;precondition(aa.requestID != bb.requestID)
  a.requirementDrafts.finish(aa,accepted:false,uncertain:true);b.requirementDrafts.finish(bb,accepted:false,uncertain:true)
  let key=CrewdPlanDrafts.Key(crew:"crew",object:"W1",source:"#1")
  var af=CrewdPlanDrafts.Fields();af.note="A W1";a.planDrafts.edit(key,af);var bf=af;bf.note="B W1";b.planDrafts.edit(key,bf)
  let hk=CrewdHumanDrafts.Key(scope:"crew",crew:"crew",ask:"H1");var hf=CrewdHumanDrafts.Fields();hf.text="A H1";a.humanDrafts.edit(hk,hf);hf.text="B H1";b.humanDrafts.edit(hk,hf)
  a.retire();await a.connect();precondition(oldCalls==1 && !a.client.connected && b.client.connected)
  precondition(a.requirementDrafts.uncertain("crew")==[aa] && b.requirementDrafts.uncertain("crew")==[bb] && b.planDrafts.fields(key).note=="B W1" && b.humanDrafts.fields(hk).text=="B H1")
  let refused=await a.perform("work.update",.object([:]));precondition(!refused)
  do {_=try await a.workStore.detail(w);preconditionFailure("retired cache issued read")}catch {precondition(!CrewdClientError.isUncertainSubmission(error))}
  _=try await b.workStore.detail(w);precondition(b.workStore.plans[w]?.value["title"] == .string("B plan") && newCalls==1)
  var release:CheckedContinuation<Void,Never>?,entered=false,lateCalls=0
  let late=CrewdWorkspace(authorityRecordID:UUID(),connect:{client in lateCalls+=1;entered=true;await withCheckedContinuation {release=$0};try await client.connect(path:aPath)})
  let connecting=Task {await late.connect()}
  while !entered {await Task.yield()};late.retire();release!.resume();await connecting.value
  await late.connect();precondition(lateCalls==1 && !late.client.connected && late.status=="已切换连接" && b.client.connected)
  b.retire()
  print("PASS: actual two authority connectors isolate identical W1/#1/H1 caches and draft UUID/payloads; retirement blocks reads/writes/reconnect; delayed connector cannot resurrect old authority")
 }
}
