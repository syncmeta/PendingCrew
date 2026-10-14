import Foundation
@main struct PlanActionsTest {
    static func main() throws {
        let source=CrewdPlanDrafts.Key(crew:"a",object:"",source:"#9007199254740993")
        let key=CrewdPlanDrafts.Key(crew:"a",object:"W9007199254740994",source:"")
        var f=CrewdPlanDrafts.Fields();f.title="original plan";f.owner="real-b-member";f.parent="W7"
        let create=try CrewdPlanCommand.build(source,f,revision:nil)
        precondition(create.0 == "work.create" && create.1["sources"] == .array([.string(source.source)]) && create.1["owner"] == .string("real-b-member") && create.1["parent"] == .string("W7"))
        f.action = .assign;f.note="handoff reason"
        let assign=try CrewdPlanCommand.build(key,f,revision:"18446744073709551615")
        precondition(assign.0 == "work.assign" && assign.1["expectedRevision"] == .string("18446744073709551615") && assign.1["note"] == .string("handoff reason"))
        f.action = .report;f.reportStatus="active";f.checkpoint=true
        do {try CrewdPlanCommand.validateOwnership(f,owner:nil);preconditionFailure("unassigned active report accepted")} catch let error as CrewdClientError {precondition(error.kind == .notSubmitted)}
        try CrewdPlanCommand.validateOwnership(f,owner:"real-b-member")
        var unassigned=f;unassigned.reportStatus="open";unassigned.checkpoint=false;try CrewdPlanCommand.validateOwnership(unassigned,owner:nil)
        unassigned.checkpoint=true
        do {try CrewdPlanCommand.validateOwnership(unassigned,owner:nil);preconditionFailure("unassigned open checkpoint accepted")} catch let error as CrewdClientError {precondition(error.kind == .notSubmitted)}
        try CrewdPlanCommand.validateOwnership(unassigned,owner:"real-b-member")
        unassigned.action = .block
        do {try CrewdPlanCommand.validateOwnership(unassigned,owner:nil);preconditionFailure("unassigned block accepted")} catch let error as CrewdClientError {precondition(error.kind == .notSubmitted)}
        let report=try CrewdPlanCommand.build(key,f,revision:"2")
        precondition(report.0 == "work.update" && report.1["status"] == .string("active") && report.1["nextCheckAt"].string != nil)
        f.checkpoint=false
        let retainedCheck=try CrewdPlanCommand.build(key,f,revision:"2",current:.object(["nextCheckAt":.string("existing authoritative checkpoint")]))
        precondition(retainedCheck.1["nextCheckAt"] == .null,"note-only active report must preserve the existing checkpoint instead of rewriting it")
        do {_ = try CrewdPlanCommand.build(key,f,revision:"2");preconditionFailure("active without check")} catch {}
        f.action = .complete;f.note="";f.evidence=""
        let done=try CrewdPlanCommand.build(key,f,revision:"3")
        precondition(done.1["status"] == .string("done") && done.1["evidence"] == .null && done.1["nextCheckAt"] == .null && done.1["note"] == .null,"completion must not impose extra evidence gate")
        f.action = .drop
        do {_ = try CrewdPlanCommand.build(key,f,revision:"3");preconditionFailure("drop without reason")} catch {}
        f.note="no longer required";let dropped=try CrewdPlanCommand.build(key,f,revision:"3")
        precondition(dropped.1["status"] == .string("dropped") && dropped.1["note"] == .string(f.note) && dropped.1["evidence"] == .null)
        f.action = .block;f.question="need decision";f.split=false
        let blocked=try CrewdPlanCommand.build(key,f,revision:"4")
        precondition(blocked.0 == "work.block" && blocked.1["on"].array[0]["type"] == .string("new_human") && blocked.1["canContinue"]["type"] == .string("nothing"))
        f.split=true;f.splitTitle="independent part";let split=try CrewdPlanCommand.build(key,f,revision:"4")
        precondition(split.1["canContinue"]["type"] == .string("split") && split.1["canContinue"]["items"].array[0]["crewId"] == .string("a") && split.1["canContinue"]["items"].array[0]["owner"] == .null,"core must inherit original owner")
        do {_ = try CrewdPlanCommand.build(key,f,revision:nil);preconditionFailure("command guessed revision")} catch {}
        var drafts=CrewdPlanDrafts();drafts.edit(key,f)
        let before=drafts.begin(key)!;precondition(drafts.begin(key) == nil)
        drafts.finish(before,accepted:false,uncertain:true)
        precondition(drafts.uncertain(key) && drafts.fields(key) == f,"unknown outcome keeps original draft")
        f.note="new payload";drafts.edit(key,f);let fresh=drafts.begin(key)!
        precondition(drafts.unknownSnapshots(key).contains{$0.version == before.version && $0.fields == before.fields},"edited draft must retain the complete original uncertain operation payload, not only its UUID")
        drafts.finish(before,accepted:true)
        precondition(drafts.busy(key) && drafts.fields(key) == f,"late old success cannot clear newer draft")
        let other=CrewdPlanDrafts.Key(crew:"b",object:"W8",source:"");var b=CrewdPlanDrafts.Fields();b.note="B draft";drafts.edit(other,b)
        drafts.finish(fresh,accepted:true);precondition(drafts.fields(other) == b,"completion cannot clear another crew")
        drafts.edit(key,f);let unknown1=drafts.begin(key)!;drafts.finish(unknown1,accepted:false,uncertain:true)
        f.note="second unknown";drafts.edit(key,f);let unknown2=drafts.begin(key)!;drafts.finish(unknown2,accepted:false,uncertain:true)
        drafts.finish(unknown1,accepted:true);precondition(drafts.uncertain(key),"old success cannot retire another unknown submission")
        drafts.finish(unknown2,accepted:true);precondition(!drafts.uncertain(key))
        print("PASS: typed create/assign/report/done/drop/block wire, exact revisions, mandatory reasons and draft ownership")
    }
}
