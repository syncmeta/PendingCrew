import Foundation
import CloudKit

@MainActor enum PendingCrewDataRoot {static let url=URL(fileURLWithPath:"/private/tmp/pcw-cloud-app-fixture-unused-shared")}
enum AppAccountFixtureFailure:Error {case unavailable}
actor AppAccountFixtureTransport:AppleAccountTransport {
    var account:AppleSyncAccount
    var identities=0,exchanges=0
    var rows:[AppleSyncAccount:[String:[UUID:AppleBranch]]]=[:]
    init(_ account:AppleSyncAccount){self.account=account}
    func accountIdentity()async throws->AppleSyncAccount{identities+=1;return account}
    func exchange(account:AppleSyncAccount,collection:String,branches:[AppleBranch])async throws->[AppleBranch]{
        exchanges+=1;var values=rows[account]?[collection] ?? [:]
        for value in branches{values[value.id]=try values[value.id].map{try AppleBranch.merge($0,value)} ?? value}
        rows[account,default:[:]][collection]=values;return Array(values.values)
    }
    func cancel()async{}
    func choose(_ value:AppleSyncAccount){account=value}
    func counts()->(Int,Int){(identities,exchanges)}
    func payloads(_ account:AppleSyncAccount)->[Data]{(rows[account] ?? [:]).values.flatMap{$0.values}.filter{!$0.retired}.compactMap(\.payload)}
}
@main struct AppAccountIntegrationFixture {
    @MainActor static func main()async throws {
        let root=URL(fileURLWithPath:CommandLine.arguments[1]),mode=CommandLine.arguments[2]
        let persistent=mode.hasPrefix("consent")
        let suite="pcw-cloud-app-fixture-"+(persistent ? appleSyncDigest(Data(root.path.utf8)):UUID().uuidString),defaults=UserDefaults(suiteName:suite)!
        defer{if mode != "consentWrite"{defaults.removePersistentDomain(forName:suite)}}
        let a=AppleSyncAccount(container:"iCloud.fixture.explicit",environment:.development,userRecordID:"synthetic-account-A")
        let b=AppleSyncAccount(container:a.container,environment:a.environment,userRecordID:"synthetic-account-B")
        let config=ApplePersonalCloudConfiguration(containerIdentifier:a.container,environment:a.environment,draftsCollection:"test.drafts",hostsCollection:"test.hosts")
        let cloud=AppAccountFixtureTransport(a)
        let key=AppleDraftKey(authorityID:"core-db-opaque",databaseID:"core-db-opaque",crewID:"crew",destination:.channel("channel"),kind:"channelMessage")
        let original=CrewComposerDraftStore.Draft(text:"  原文 Todo @机长\n",stagedMentions:[.init(token:"@机长",mention:.captain)],replyTarget:.init(mention:.captain,replyToId:"original",quotedSender:"机长",quotedSnippet:"原引用"))
        if mode=="retained" {
            try FileManager.default.createDirectory(at:root.appendingPathComponent("latest-input.json"),withIntermediateDirectories:true)
        }
        let owner=CrewPersonalDraftService.owned(root:root,deviceIdentity:{if mode=="retained"{throw AppAccountFixtureFailure.unavailable};return "fixture-device"},configuration:config,transportFactory:{_ in cloud},defaults:defaults)
        let same=CrewPersonalDraftService.owned(root:root,deviceIdentity:{preconditionFailure("duplicate owner identity")})
        precondition(owner===same)
        if mode=="returnRead" {
            try await owner.enablePersonalSync()
            precondition(owner.localDraft(key)?.text=="A current newer")
            var late=original;late.text="A old editor late original 2"
            precondition(owner.recovered.values.contains(late),"whole late original must survive independent process reopen")
            print("PASS reselected-account late original durable full-payload recovery reopen");return
        }
        if mode=="latestWrite"{owner.checkpoint(original,key:key);print("PASS actual latest whole payload saved before branch task");return}
        if mode=="latestRead"{precondition(owner.catalog.contains(key) && owner.localDraft(key)==original);print("PASS actual latest-only process reopen catalog without account or Keychain");return}
        if mode=="consentWrite" {
            try await owner.enablePersonalSync();precondition(owner.cloudOptedIn);defaults.synchronize()
            print("PASS actual consent saved without automatic local copy");return
        }
        if mode=="consentRead" {
            precondition(owner.cloudOptedIn);CrewPersonalDraftService.setApplicationActive(true)
            try await Task.sleep(for:.milliseconds(400));let counts=await cloud.counts()
            precondition(counts.0==1 && counts.1>0,"consented fresh process must use the same enable+exchange path")
            print("PASS persisted consent fresh process active account verification and synchronization");return
        }
        if mode=="nil" {
            let nilRoot=root.appendingPathComponent("nil-owner")
            let nilOwner=CrewPersonalDraftService.owned(root:nilRoot,deviceIdentity:{"fixture-device"},configuration:nil,transportFactory:{_ in preconditionFailure("nil qualification constructed transport")},defaults:defaults)
            CrewPersonalDraftService.setApplicationActive(true)
            do{try await nilOwner.enablePersonalSync();preconditionFailure("unqualified enabled")}catch{}
            nilOwner.setPreferenceSync(true)
            precondition(!nilOwner.cloudConfigured && !nilOwner.preferencesQualified)
            print("PASS actual nil configuration blocks Cloud and KVS even with preference consent");return
        }
        if mode=="active" {
            CrewPersonalDraftService.setApplicationActive(true)
            try await Task.sleep(for:.milliseconds(50));let untouched=await cloud.counts();precondition(untouched==(0,0),"default off must not probe account")
            try await owner.enablePersonalSync();owner.checkpoint(original,key:key)
            try await Task.sleep(for:.milliseconds(650));let first=await cloud.counts();precondition(first.0==1 && first.1>0)
            try await Task.sleep(for:.milliseconds(250));let settled=await cloud.counts();precondition(settled==first,"projection refresh must not enqueue endless exchanges")
            await owner.disablePersonalSync();let stopped=await cloud.counts();CrewPersonalDraftService.setApplicationActive(false);CrewPersonalDraftService.setApplicationActive(true)
            try await Task.sleep(for:.milliseconds(50));let after=await cloud.counts();precondition(after==stopped && !owner.cloudOptedIn)
            print("PASS actual App owner default-off, persisted explicit opt-in, active checkpoint coalescing and disabled foreground");return
        }
        owner.checkpoint(original,key:key);let local=owner.editingScope(),old=owner.snapshot(key,payload:original)!
        if mode=="merge" {
            try await Task.sleep(for:.milliseconds(350));await owner.refresh(key)
            let retiring=Set((owner.versions[key] ?? []).map(\.id))
            precondition(!retiring.isEmpty)
            owner.checkpoint(original,key:key);let expected=owner.snapshot(key,payload:original)!
            let merging=Task{try await owner.resolve(key,payload:.init(text:"stale merge"),retiring:retiring,expected:expected)}
            for _ in 0..<5{await Task.yield()}
            owner.checkpoint(.init(text:"new input during original writer await"),key:key)
            do{_ = try await merging.value;preconditionFailure("stale merge overwrote concurrent input")}catch{}
            precondition(owner.localDraft(key)?.text=="new input during original writer await")
            print("PASS actual Namespace resolve writer-await rechecks original full ticket");return
        }
        if mode=="retained" {
            precondition(CrewPersonalDraftService.hasUnsavedInProcess && CrewPersonalDraftService.unsavedExitToken != nil)
            let ticket=owner.exportTicket(key,payload:original)!
            try await owner.enablePersonalSync()
            precondition(owner.unsavedLatest.isEmpty && CrewPersonalDraftService.hasUnsavedInProcess,"old failed originals remain in exit guard")
            owner.checkpoint(.init(text:"old unsaved newer"),key:key,scope:local)
            owner.acknowledgeExport(ticket)
            precondition(CrewPersonalDraftService.hasUnsavedInProcess,"old export cannot acknowledge a newer retained original")
            owner.acknowledgeExport(ticket)
            let recovery=owner.staleExportTicket(key,payload:.init(text:"old unsaved newer"),scope:local)!
            owner.checkpoint(.init(text:"old unsaved newest"),key:key,scope:local)
            owner.acknowledgeRecoveredExport(recovery)
            precondition(CrewPersonalDraftService.hasUnsavedInProcess,"an older recovery export must not release newer input")
            let fresh=owner.staleExportTicket(key,payload:.init(text:"old unsaved newest"),scope:local)!
            owner.acknowledgeRecoveredExport(fresh)
            precondition(!CrewPersonalDraftService.hasUnsavedInProcess)
            print("PASS retained old namespace failed latest remains in exact export/quit guard");return
        }
        let host=try ApplePortableHost(displayName:"synthetic public hint",endpoint:URL(string:"tls://154.36.143.228:15443")!,transportAuthorityID:"synthetic-transport",databaseID:"core-db-opaque",verifiedSPKI:String(repeating:"a",count:64))
        await owner.rememberHost(host)
        let hint=local.owner.hostTicket(host)!
        try await owner.enablePersonalSync();precondition(owner.localDraft(key)==nil,"opt-in cannot copy local text")
        precondition(owner.hosts.isEmpty,"account discovery cannot move the local directory")
        try await owner.copyHost(hint,from:local.owner)
        precondition(owner.hosts==[host],"only an explicit typed hint selection may copy")
        let accountScope=owner.editingScope();owner.checkpoint(.init(text:"account A"),key:key)
        precondition(!owner.clearIfUnchanged(old))
        owner.checkpoint(.init(text:"late local"),key:key,scope:local)
        precondition(owner.localDraft(key)?.text=="account A" && local.owner.localDraft(key)==original && local.owner.recovered.values.contains(where:{$0.text=="late local"}))
        let sent=owner.snapshot(key,payload:.init(text:"account A"))!
        NotificationCenter.default.post(name:.CKAccountChanged,object:nil)
        precondition(!owner.isCurrent(sent),"notification must fence before MainActor projection")
        owner.checkpoint(.init(text:"late A"),key:key,scope:accountScope)
        for _ in 0..<8{await Task.yield()}
        await cloud.choose(b);try await owner.enablePersonalSync()
        precondition(owner.localDraft(key)==nil && accountScope.owner.localDraft(key)?.text=="account A" && accountScope.owner.recovered.values.contains(where:{$0.text=="late A"}))
        owner.checkpoint(.init(text:"account B"),key:key)
        precondition(!owner.clearIfUnchanged(sent))
        let source=owner.retainedExportTicket(local.owner,key:key)!,target=owner.snapshot(key,payload:.init(text:"account B"))!
        owner.checkpoint(.init(text:"B newer"),key:key)
        do{try owner.copyRetained(source,replacing:target);preconditionFailure("stale nonempty target accepted")}catch{}
        precondition(owner.localDraft(key)?.text=="B newer")
        owner.checkpoint(original,key:key)
        precondition(owner.snapshot(key,payload:original,scope:local)==nil,"same bytes cannot mint a new-account ticket for an old composer")
        await owner.disablePersonalSync()
        owner.checkpoint(.init(text:"local ABA replacement"),key:key)
        await cloud.choose(b);try await owner.enablePersonalSync()
        owner.checkpoint(original,key:key)
        do{try owner.copyRetained(source,replacing:owner.snapshot(key,payload:original));preconditionFailure("stale retained source accepted")}catch{}
        if mode=="return" || mode=="returnWrite" {
            await cloud.choose(a);try await owner.enablePersonalSync()
            owner.checkpoint(.init(text:"A current newer"),key:key)
            var late=original;late.text="A old editor late original"
            owner.checkpoint(late,key:key,scope:accountScope)
            precondition(owner.localDraft(key)?.text=="A current newer","old A scope overwrote reselected A latest")
            precondition(owner.recovered.values.contains(where:{$0.text=="A old editor late original"}),"late original must remain visible for export")
            let count=owner.recovered.count
            late.text="A old editor late original 2"
            owner.checkpoint(late,key:key,scope:accountScope)
            precondition(owner.recovered.count==count && owner.recovered.values.contains(late),"same stale editor must coalesce, preserving all structured fields")
            try await owner.synchronizePersonal()
            let sent=await cloud.payloads(a)
            precondition(!sent.contains{(try? JSONDecoder().decode(CrewComposerDraftStore.Draft.self,from:$0))==late},"stale originals are local recovery, not automatic Cloud branches")
            print("PASS actual A to B to A old editor retained without replacing selected latest or uploading late original");return
        }
        print("PASS actual service namespace projection, original clear CAS, synchronous account fence and explicit target copy CAS")
    }
}
