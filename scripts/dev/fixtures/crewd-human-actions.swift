import Foundation
@MainActor final class HumanFixture {
    var calls:[(String,CrewdValue)]=[]
    var invalid:String?
    var lost=false
    var open=true
    func call(_ method:String,_ params:CrewdValue) async throws->CrewdValue {
        calls.append((method,params))
        if method=="work.list"{return .object(["items":.array([]),"nextAfter":.null])}
        if method=="todo.list"{return .array([])}
        if method=="ask.list"{return open ? .array([.object(["ask":.object(["ask":.string("H42"),"crewId":.string("b"),"question":.string("actual current question"),"state":.string("open")])])]):.array([])}
        if lost{throw CrewdClientError(message:"actual submitted response lost")}
        let withdrawn=method=="ask.withdraw",field=withdrawn ? "withdrawReason":"answer",input=withdrawn ? "reason":"text"
        var receipt:[String:CrewdValue]=["ask":.string("H42"),"crewId":.string("b"),"state":.string(withdrawn ? "withdrawn":"answered"),field:.string(params[input].string!.trimmingCharacters(in:.whitespacesAndNewlines))]
        if let invalid{receipt[invalid] = .string("incorrect receipt")}
        open=false
        return .object(receipt)
    }
}
@main struct Test {
    @MainActor static func main() async throws {
        let a=CrewdHumanDrafts.Key(scope:"a",crew:"b",ask:"H42"),b=CrewdHumanDrafts.Key(scope:"another-window",crew:"b",ask:"H42")
        var d=CrewdHumanDrafts(),text=CrewdHumanDrafts.Fields();text.text="  exact original response\n";d.edit(a,text)
        let first=d.begin(a)!;precondition(d.begin(a)==nil,"busy cannot submit a second command")
        d.finish(first,accepted:false,uncertain:true)
        precondition(d.uncertain(a)==[first] && d.fields(a)==text,"unknown keeps original payload")
        text.text="edited while earlier outcome unknown";d.edit(a,text);precondition(d.uncertain(a)==[first],"editing must not erase uncertain earlier write")
        var other=CrewdHumanDrafts.Fields();other.action = .withdraw;other.text="another window reason";d.edit(b,other)
        let fresh=d.begin(a)!;d.finish(first,accepted:true);precondition(d.busy(a) && d.fields(a)==text && d.fields(b)==other,"late old success cannot clear newer/sibling window draft")
        d.finish(fresh,accepted:false,uncertain:true);let pending=d.uncertain(a);precondition(pending==[fresh]);d.finish(first,accepted:true);precondition(d.uncertain(a)==pending,"old receipt cannot retire newer unknown")
        for withdrawn in [false,true] {
            let f=HumanFixture(),s=CrewdWorkStore(call:{try await f.call($0,$1)});s.selectCrew("a");try await s.refresh()
            let result=try await s.resolveHuman(CrewdWorkID("H42",kind:.human)!,crew:"b",text:"  deliberate value\n",withdraw:withdrawn)
            precondition(result[withdrawn ? "withdrawReason":"answer"] == .string("deliberate value"),"core trim matches explicit receipt")
            precondition(s.humanTasks.isEmpty,"resolved question reconciles actual inbox")
            precondition(f.calls.filter{$0.0 == (withdrawn ? "ask.withdraw":"ask.answer")}.count==1,"one explicit write")
        }
        for field in ["ask","crewId","state","answer"] {
            let f=HumanFixture(),s=CrewdWorkStore(call:{try await f.call($0,$1)});s.selectCrew("a");f.invalid=field
            do {_ = try await s.resolveHuman(CrewdWorkID("H42",kind:.human)!,crew:"b",text:"reply",withdraw:false);preconditionFailure("fake receipt accepted: \(field)")}catch{}
            precondition(f.calls.count==1,"invalid receipt cannot trigger automatic retry/read-based success")
        }
        let f=HumanFixture(),s=CrewdWorkStore(call:{try await f.call($0,$1)});s.selectCrew("a");f.lost=true
        do {_ = try await s.resolveHuman(CrewdWorkID("H42",kind:.human)!,crew:"b",text:"submitted response",withdraw:false);preconditionFailure()}catch{}
        let count=f.calls.filter{$0.0 == "ask.answer"}.count
        f.lost=false;s.invalidateConnection();try await s.refresh();precondition(f.calls.filter{$0.0 == "ask.answer"}.count==count,"reconnect only reads unknown answer")
        print("PASS: scoped Human draft snapshots, busy/late outcomes, exact receipt proof/trim and no unknown write replay")
    }
}
