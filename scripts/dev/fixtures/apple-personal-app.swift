import Foundation
import Darwin

@main struct PersonalAppRegression {
    @MainActor static func main()async throws {
        let root=URL(fileURLWithPath:CommandLine.arguments[1],isDirectory:true)
        let mode=CommandLine.arguments.dropFirst(2).first ?? "write"
        let read=mode == "read"
        let key=AppleDraftKey(authorityID:"actual-core-db",databaseID:"actual-core-db",crewID:"crew",destination:.channel("channel"),kind:"channelMessage")
        let original=CrewComposerDraftStore.Draft(text:"  原文 Todo @机长🌙\n",stagedMentions:[.init(token:"@机长",mention:.captain)],replyTarget:.init(mention:.captain,replyToId:"original-message",quotedSender:"机长",quotedSnippet:"原引用"))
        let a=CrewPersonalDraftService.owned(root:root,deviceIdentity:{"fixture-device"})
        let b=CrewPersonalDraftService.owned(root:root,deviceIdentity:{preconditionFailure("second window must not construct another actor/device owner")})
        precondition(a === b,"multiple windows reuse one App owner")
        if mode == "latestOnly"{a.checkpoint(original,key:key);exit(0)}
        if mode == "catalog"{await a.loadCatalog();precondition(a.catalog.contains(key),"latest-only durable original must be discoverable offline before branch write");precondition(a.localDraft(key) == original);print("PASS latest-only offline catalog and full original");return}
        if mode == "lateClear"{
            a.checkpoint(original,key:key)
            let sent=a.snapshot(key,payload:original)!
            b.checkpoint(.init(text:"另一窗口的新原文"),key:key)
            precondition(!a.clearIfUnchanged(sent))
            precondition(a.localDraft(key)?.text == "另一窗口的新原文","late receipt must not clear another window's personal original")
            b.checkpoint(original,key:key)
            precondition(!a.clearIfUnchanged(sent),"identical text with a new revision is not the sent draft")
            let current=a.snapshot(key,payload:original)!
            precondition(a.clearIfUnchanged(current))
            print("PASS shared personal CAS protects another window and ABA; exact current receipt clears")
            return
        }
        if mode == "mergeRace"{
            a.checkpoint(original,key:key)
            let ticket=a.snapshot(key,payload:original)!
            let merge=Task{try await a.resolve(key,payload:.init(text:"旧确认合并"),retiring:[],expected:ticket)}
            await Task.yield()
            b.checkpoint(.init(text:"跨等待的新原文"),key:key)
            do{try await merge.value;preconditionFailure("stale merge accepted")}catch{}
            precondition(a.localDraft(key)?.text == "跨等待的新原文")
            print("PASS original merge confirmation rejects a newer shared revision across writer await")
            return
        }
        if read {
            let restored=await a.restoreOwn(key)
            precondition(restored == original,"real whole Draft survives process reopening with exact text, mentions and reply")
            print("PASS original structured Draft across process reopening; no Keychain or cloud calls")
            return
        }
        a.checkpoint(original,key:key)
        while a.saving.contains(key){await Task.yield()}
        precondition(a.localDraft(key) == original && a.errors[key] == nil)
        await a.refresh(key)
        precondition(a.versions[key]?.count == 1 && a.versions[key]?.first?.payload == original)
        let second=AppleDraftKey(authorityID:"another-db",databaseID:"another-db",crewID:"crew",destination:.channel("channel"),kind:"channelMessage")
        b.checkpoint(.init(text:"另一个来源"),key:second)
        while b.saving.contains(second){await Task.yield()}
        precondition(a.localDraft(key) == original,"other source cannot replace current input")
        let snapshot=a.exportTicket(key,payload:original)!
        // This real permission mismatch rejects both original latest-file and
        // branch persistence while leaving the last durable files untouched.
        try FileManager.default.setAttributes([.posixPermissions:0o755],ofItemAtPath:root.path)
        let edited=CrewComposerDraftStore.Draft(text:"写失败仍保留的原文")
        a.checkpoint(edited,key:key)
        while a.saving.contains(key){await Task.yield()}
        precondition(a.localDraft(key) == edited && a.unsavedLatest.contains(key) && a.errors[key] != nil && CrewPersonalDraftService.unsavedExitToken != nil)
        a.acknowledgeExport(snapshot)
        precondition(a.unsavedLatest.contains(key),"old exported version must not clear a later edit")
        let latest=a.exportTicket(key,payload:edited)!
        let bytes=try a.export(key)
        let exported=try JSONDecoder().decode(CrewComposerDraftStore.Draft.self,from:bytes);precondition(exported == edited)
        a.acknowledgeExport(latest)
        precondition(!a.unsavedLatest.contains(key),"only exact explicit export confirms external durable copy")
        try FileManager.default.setAttributes([.posixPermissions:0o700],ofItemAtPath:root.path)
        a.checkpoint(original,key:key)
        while a.saving.contains(key){await Task.yield()}
        precondition(a.localDraft(key) == original && !a.unsavedLatest.contains(key))
        let spki=String(repeating:"a",count:64)
        for raw in ["tls://154.36.143.228:7443","tls://192.168.1.2:7443","tls://[2001:db8::12]:7443","tls://mac.local:7443"] {
            _ = try ApplePortableHost(displayName:"host",endpoint:URL(string:raw)!,transportAuthorityID:"transport-only",databaseID:"actual-core-db",verifiedSPKI:spki)
        }
        for raw in ["wss://core.example.com:7443","tls://core.example.com","tls://core.example.com:0","tls://localhost:7443","tls://127.0.0.1:7443","tls://[::1]:7443","tls://user:secret@core.example.com:7443","tls://core.example.com:7443/?token=secret"] {
            do{_ = try ApplePortableHost(displayName:"host",endpoint:URL(string:raw)!,transportAuthorityID:"transport-only",databaseID:"db",verifiedSPKI:spki);preconditionFailure("unsafe or incorrect-protocol directory endpoint accepted")}
            catch AppleSyncFailure.invalid{}
        }
        print("PASS actual shared App owner, real full Draft, source isolation, failed local save preservation, original export/ABA and actual TLS directory; cloud disabled")
    }
}
