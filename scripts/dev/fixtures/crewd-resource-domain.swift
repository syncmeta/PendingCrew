import Foundation
@main struct ResourceDomainTest {
    static func main() {
        var drafts=CrewdResourceDrafts()
        let document=CrewdResourceDrafts.Key(crew:"a",view:.documents,object:"",operation:"create",domain:.document)
        let folder=CrewdResourceDrafts.Key(crew:"a",view:.documents,object:"",operation:"create",domain:.folder)
        var doc=CrewdResourceDrafts.Fields();doc.title="private document original";doc.body="exact original document body";drafts.edit(document,doc)
        let unknown=drafts.begin(document)!;drafts.finish(unknown,accepted:false,uncertain:true)
        precondition(drafts.fields(folder).title.isEmpty && drafts.uncertain(folder).isEmpty,"document input/unknown request must not become the folder create input or request")
        var group=CrewdResourceDrafts.Fields();group.title="folder original";drafts.edit(folder,group)
        let folderRequest=drafts.begin(folder)!
        precondition(folderRequest.request != unknown.request && drafts.fields(document)==doc)
        drafts.finish(unknown,accepted:true)
        precondition(drafts.busy(folder) && drafts.fields(folder)==group,"late document receipt cannot clear a pending folder write")
        drafts.finish(folderRequest,accepted:true)
        print("PASS: document/folder create draft domains, unknown UUIDs and late receipts remain independent")
    }
}
