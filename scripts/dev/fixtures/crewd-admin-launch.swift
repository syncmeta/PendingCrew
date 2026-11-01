import Foundation
@main struct AdminLaunchFixture {
 @MainActor static func main() async throws {
  func row(_ id:String="AC00000000000000000001",crew:String="a")->CrewdValue {.object(["id":.string(id),"crewId":.string(crew),"revision":.string("v"),"createdAt":.integer(1),"source":.object(["sessionId":.string("s"),"budgetEpoch":.integer(1),"restoreEpoch":.string("e"),"announcementId":.string("m")]),"targets":.array([.object(["id":.string("W1"),"crewId":.string("a"),"revision":.string("1"),"kind":.string("overdue"),"scope":.string("in_scope")])]),"decision":.string("observed"),"decisionRecord":.null,"execution":.string("not_started"),"executionRecord":.null,"retirement":.string("not_started"),"retirementRecord":.null,"outcomeRecord":.null,"handoffRecord":.null])}

 func change(_ v:CrewdValue,_ key:String,_ value:CrewdValue)->CrewdValue {guard case .object(var d)=v else{fatalError()};d[key]=value;return .object(d)}
 var response=row()
 let store=CrewdWorkStore {method,_ in precondition(method=="adminCase.get");return response}
 store.selectCrew("a");store.configureCapabilities(["adminCase.automaticExecution"])
 do {_ = try await store.adminCase(id:"AC00000000000000000001");fatalError("new capability must require launchRecord")}catch{}
 response=change(row(),"launchRecord",.null)
 _ = try await store.adminCase(id:"AC00000000000000000001")
 let waiting:CrewdValue = .object(["phase":.string("waiting"),"reason":.string("capacity_unavailable"),"revision":.string("schedule-r"),"decisionId":.string("d"),"updatedAt":.integer(1),"sessionId":.null,"attemptId":.null])
 response=change(row(),"launchRecord",waiting)
 _ = try await store.adminCase(id:"AC00000000000000000001")
 for malformed in [change(waiting,"phase",.string("success")),change(waiting,"reason",.string("unrecognized")),change(waiting,"phase",.string("bound")),change(waiting,"attemptId",.string("foreign"))] {
  response=change(row(),"launchRecord",malformed)
  do {_ = try await store.adminCase(id:"AC00000000000000000001");fatalError("invalid scheduling evidence accepted")}catch{}
 }
 response=change(row(),"launchRecord",change(change(waiting,"phase",.string("unknown")),"reason",.string("restart_unknown")))
 _ = try await store.adminCase(id:"AC00000000000000000001")
 let execution:CrewdValue = .object(["phase":.string("unknown"),"reason":.string("restart_unknown"),"sessionId":.string("admin"),"attemptId":.string("attempt"),"effectsCount":.integer(0),"generation":.null])
 let bound=change(change(change(change(waiting,"phase",.string("bound")),"reason",.string("bound")),"sessionId",.string("admin")),"attemptId",.string("attempt"))
 response=change(change(change(row(),"execution",.string("unknown")),"executionRecord",execution),"launchRecord",bound)
 _ = try await store.adminCase(id:"AC00000000000000000001")
 response=change(response,"launchRecord",change(bound,"sessionId",.string("different-admin")))
 do {_ = try await store.adminCase(id:"AC00000000000000000001");fatalError("wrong binding adopted")}catch{}
 store.configureCapabilities([]);response=row()
 _ = try await store.adminCase(id:"AC00000000000000000001")
 print("administrative scheduling fixture passed")
 }
}
