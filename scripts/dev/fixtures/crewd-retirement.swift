import Foundation
@main struct RetirementFixture {
 static func main() throws {
  var row:[String:CrewdValue] = ["decisionId":.string("d"),"phase":.string("exit_requested"),"reason":.string("answered"),"generation":.string("18446744073709551615"),"probability":.number(0.9),"adopted":.bool(false)]
  func response(_ row:CrewdValue,crew:String="a")->CrewdValue {.object(["crewId":.string(crew),"sessionId":.string("s"),"record":row])}
  _ = try CrewdRetirementRecord.decode(response(.null),crew:"a",session:"s")
  for phase in ["prepared","dispatched","decision_ready","exit_requested","parked","stale","unknown","suppressed"] {
   row["phase"] = .string(phase)
   _ = try CrewdRetirementRecord.decode(response(.object(row)),crew:"a",session:"s")
  }
  for key in ["decisionId","generation","phase","reason","probability","adopted"] {
   var bad=row;bad.removeValue(forKey:key)
   do {_ = try CrewdRetirementRecord.decode(response(.object(bad)),crew:"a",session:"s");fatalError("missing accepted")} catch {}
  }
  for (key,value):(String,CrewdValue) in [("generation",.string("01")),("probability",.number(2)),("phase",.string("done")),("adopted",.string("true"))] {
   var bad=row;bad[key]=value
   do {_ = try CrewdRetirementRecord.decode(response(.object(bad)),crew:"a",session:"s");fatalError("invalid accepted")} catch {}
  }
  do {_ = try CrewdRetirementRecord.decode(response(.object(row),crew:"b"),crew:"a",session:"s");fatalError("foreign accepted")} catch {}
  print("retirement phases, null, foreign scope, missing and malformed fields passed")
 }
}
