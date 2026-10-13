import Foundation
@main struct RequirementDraftsTest {
    static func main() {
        var drafts=CrewdRequirementDrafts()
        drafts.set("a",text:"original A");drafts.set("b",text:"original B")
        let a=drafts.begin("a")!;precondition(drafts.begin("a") == nil,"duplicate submit must be blocked")
        drafts.finish(a,accepted:false,uncertain:true)
        let retry=drafts.begin("a")!;precondition(retry.requestID == a.requestID && retry.text == a.text,"unknown outcome keeps identity and original payload")
        drafts.finish(retry,accepted:false)
        drafts.set("a",text:"edited A")
        precondition(drafts.uncertain("a") == [a],"editing must retain original unknown snapshot")
        let edited=drafts.begin("a")!;precondition(edited.requestID != a.requestID,"changed payload starts new request")
        drafts.finish(a,accepted:true)
        precondition(drafts.text("a") == "edited A" && drafts.busy("a"),"old completion cannot clear new payload or its submit gate")
        precondition(drafts.text("b") == "original B","A result never clears B")
        drafts.finish(edited,accepted:true)
        precondition(drafts.uncertain("a").isEmpty,"explicit old success retires its unknown witness")
        precondition(drafts.text("a").isEmpty && !drafts.busy("a") && drafts.text("b") == "original B")
        drafts.set("a",text:"one");let first=drafts.begin("a")!;drafts.finish(first,accepted:false)
        drafts.set("a",text:"two");drafts.set("a",text:"one");let second=drafts.begin("a")!
        precondition(first.requestID != second.requestID,"editing away and back cannot reuse an unknown request identity")
        drafts.finish(second,accepted:false,uncertain:true)
        precondition(drafts.confirm(second),"explicit user confirmation reuses retained old request")
        precondition(!drafts.confirm(second),"confirmation cannot submit twice")
        drafts.finish(second,accepted:true)
        precondition(drafts.uncertain("a").isEmpty)
        print("PASS: per-crew payload/UUID ownership, pending gate, unknown outcome retention and exact completion clearing")
    }
}
