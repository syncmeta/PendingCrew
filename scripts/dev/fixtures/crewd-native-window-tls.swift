    @MainActor private static func actualWindows(pairing: CrewdRemotePairing, recordID: UUID, directory: URL, grantID: String) async throws {
        func check(_ value: Bool, _ message: String) throws {
            if !value { throw NSError(domain: "ActualWindowTLS", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
        }
        func until(_ value: () -> Bool, _ message: String) async throws {
            let stop=Date().addingTimeInterval(8)
            while !value(), Date()<stop { try await Task.sleep(nanoseconds: 1_000_000) }
            try check(value(),message)
        }
        let namespace=try CrewdPresentationJournal.localNamespace(directory: directory)
        var locals: [CrewdWorkspace]=[]
        for _ in 0..<4 {
            let local=CrewdWorkspace(dataDirectory: directory,authorityRecordID: namespace)
            try await local.client.connect(path:directory.appendingPathComponent("crewd.sock").path)
            locals.append(local)
        }
        var nextA=[locals[0],locals[2]], nextB=[locals[1],locals[3]]
        var factoriesA=CrewdRemoteWindow.Factories(), factoriesB=CrewdRemoteWindow.Factories()
        factoriesA.local={_,_ in precondition(!nextA.isEmpty);return nextA.removeFirst()}
        factoriesB.local={_,_ in precondition(!nextB.isEmpty);return nextB.removeFirst()}
        // Catalog persistence is not an authentication gate. This fixture uses
        // loopback and must not call the user's personal host/CloudKit service.
        factoriesA.rememberHost={_,_ in}; factoriesB.rememberHost={_,_ in}
        let a=CrewdRemoteWindow(pairing:pairing,factories:factoriesA)
        let b=CrewdRemoteWindow(pairing:pairing,factories:factoriesB)
        let modelA=AppModel(window:a,directory:{directory},bindDrafts:{_ in})
        let modelB=AppModel(window:b,directory:{directory},bindDrafts:{_ in})
        defer {a.close();b.close();for local in locals {local.retire()}}
        await modelA.connectCrewdAuthority();await modelB.connectCrewdAuthority()
        try await until({modelA.crewdBackend?.workspace === locals[0] && modelB.crewdBackend?.workspace === locals[1]},"original Local backends did not qualify")
        try check(locals[0].originalJournalReady && locals[1].originalJournalReady,"real Local history gate failed")
        let context=CrewdDrafts.Context.message(crew:"never-submitted-crew",channel:"never-submitted-channel",session:"")
        locals[0].messageDrafts.set("LOCAL A UNSENT 中文",for:context)
        locals[1].messageDrafts.set("LOCAL B UNSENT",for:context)
        let root=directory.appendingPathComponent("frontend-presentation/unknown-submissions")
        let localJournal=try CrewdPresentationJournal(root:root,scope:locals[0].presentationScope!)
        let localUnknown=try localJournal.prepare(method:"session.resume",params:.object(["sessionId":.string("never-submitted-session")]),crew:"never-submitted-crew")
        await modelA.connectCrewdAuthority(computerID:recordID)
        await modelB.connectCrewdAuthority(computerID:recordID)
        try await until({a.selected==recordID && b.selected==recordID && modelA.crewdBackend?.workspace === a.workspace && modelB.crewdBackend?.workspace === b.workspace},"real TLS candidates did not commit original App backends")
        guard let remoteA=a.workspace,let remoteB=b.workspace else {fatalError("missing actual remote workspaces")}
        try check(remoteA !== remoteB && remoteA.client.readyForWork && remoteB.client.readyForWork,"windows reused a workspace or skipped real TLS hello")
        try check(remoteA.dataDirectory==nil && remoteB.dataDirectory==nil && remoteA.originalJournalReady && remoteB.originalJournalReady,"remote original history was not qualified")
        try check(remoteA.client.authorityBinding["grantId"].string==grantID && remoteB.client.authorityBinding["grantId"].string==grantID,"real remote grant binding was lost")
        try check(remoteA.presentationScope != locals[0].presentationScope,"Local and authenticated grant history share authority")
        try check(locals[0].isRetired && locals[1].isRetired,"old Local authority remained active")
        try check(locals[0].messageDrafts.text(for:context)=="LOCAL A UNSENT 中文" && locals[1].messageDrafts.text(for:context)=="LOCAL B UNSENT","Local drafts changed during actual remote choice")
        try check(try localJournal.unknownRecords().contains{$0.id==localUnknown.id},"Local Unknown was lost or marked completed")
        remoteA.messageDrafts.set("REMOTE A UNSENT",for:context)
        let remoteJournal=try CrewdPresentationJournal(root:root,scope:remoteA.presentationScope!)
        let remoteUnknown=try remoteJournal.prepare(method:"session.resume",params:.object(["sessionId":.string("never-submitted-remote-session")]),crew:"never-submitted-crew")
        await modelA.connectCrewdAuthority()
        try await until({a.selected==nil && a.workspace === locals[2] && modelA.crewdBackend?.workspace === locals[2]},"return to real Local did not commit")
        try check(locals[2].originalJournalReady && locals[2].client.readyForWork,"returned Local bypassed original gates")
        try check(b.workspace === remoteB && b.selected==recordID && remoteB.client.readyForWork && remoteB.originalJournalReady,"A's Local choice disturbed B's actual TLS authority")
        try check(remoteA.isRetired && remoteA.messageDrafts.text(for:context)=="REMOTE A UNSENT","retired remote draft was lost")
        try check(a.retainedWorkspaces.contains{$0 === remoteA} && a.retainedWorkspaces.contains{$0 === locals[0]},"original input review lost prior authority")
        try check(try remoteJournal.unknownRecords().contains{$0.id==remoteUnknown.id},"remote Unknown was replayed or removed")
        try check(try localJournal.unknownRecords().contains{$0.id==localUnknown.id},"returning Local changed original Unknown")
        try check(locals[2].presentationScope==locals[0].presentationScope,"returning Local changed the original namespace")
        await modelB.connectCrewdAuthority()
        try await until({b.workspace === locals[3] && modelB.crewdBackend?.workspace === locals[3]},"B failed independent return to real Local")
        try check(remoteB.isRetired && !a.isSelecting && !b.isSelecting,"old remote or selection state remained active")
        stage("actual App two windows Local/mTLS/Local retained original drafts and Unknown")
    }
