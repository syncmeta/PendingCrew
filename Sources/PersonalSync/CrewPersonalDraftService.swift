import Foundation
import Combine
import Security
import Darwin

/// One owner per original canonical root. Cloud is personal data only, never Core
/// authorization, a message sender, session runner or background worker.
@MainActor final class CrewPersonalDraftService:ObservableObject {
    typealias Payload=CrewComposerDraftStore.Draft
    typealias Namespace=ApplePersonalAccountNamespace<Payload>
    typealias Coordinator=ApplePersonalAccountCoordinator<Payload>
    struct Version:Identifiable {let id:UUID;let deviceID:String;let own:Bool;let payload:Payload}
    struct EditingScope { let owner:Namespace;let projection:UUID;let editorID=UUID() }
    struct Snapshot {let owner:Namespace;let value:Namespace.Ticket;var key:AppleDraftKey{value.key}}
    struct ExportTicket {let owner:Namespace;let value:Namespace.Ticket}
    struct RecoveredExportTicket {let owner:Namespace;let value:Namespace.RecoveredTicket}
    private static var instance:CrewPersonalDraftService?
    private static var owners:[String:CrewPersonalDraftService]=[:]
    private static var appActive=false
    // Filled only by a later explicitly qualified signed overlay. Nil performs
    // no CKContainer/account/KVS discovery, even if a prior preference is true.
    static let applicationConfiguration:ApplePersonalCloudConfiguration?=nil
    static var shared:CrewPersonalDraftService {
        if let instance{return instance}
        let next=owned(root:PendingCrewDataRoot.url.appendingPathComponent("personal-input",isDirectory:true))
        instance=next;return next
    }
    static var hasUnsavedInProcess:Bool{owners.values.contains{$0.coordinator.hasUnsavedOriginals}}
    static var unsavedExitToken:String? {
        let tokens=owners.sorted{$0.key < $1.key}.compactMap{path,owner in owner.coordinator.unsavedExitToken.map{path+"/"+$0}}
        return tokens.isEmpty ? nil:appleSyncDigest(Data(tokens.joined(separator:"\n").utf8))
    }
    static func setApplicationActive(_ active:Bool){
        appActive=active
        for owner in owners.values{owner.setActive(active)}
    }
    static func owned(root:URL,deviceIdentity:(@Sendable ()async throws->String)?=nil,
                      configuration:ApplePersonalCloudConfiguration?=applicationConfiguration,
                      transportFactory:((ApplePersonalCloudConfiguration)throws->any AppleAccountTransport)?=nil,
                      defaults:UserDefaults = .standard)->CrewPersonalDraftService {
        var canonical=root
        do{try FileManager.default.createDirectory(at:root,withIntermediateDirectories:true,attributes:[.posixPermissions:0o700]);if let value=realpath(root.path,nil){canonical=URL(fileURLWithPath:String(cString:value),isDirectory:true);free(value)}}catch{}
        if let value=owners[canonical.path]{return value}
        let value=CrewPersonalDraftService(root:canonical,deviceIdentity:deviceIdentity,configuration:configuration,transportFactory:transportFactory,defaults:defaults)
        owners[canonical.path]=value;value.setActive(appActive);return value
    }
    @Published private(set) var versions:[AppleDraftKey:[Version]]=[:]
    @Published private(set) var errors:[AppleDraftKey:String]=[:]
    @Published private(set) var initializationError:String?
    @Published private(set) var status="草稿仅本机保存"
    @Published private(set) var saving=Set<AppleDraftKey>()
    @Published private(set) var unsavedLatest=Set<AppleDraftKey>()
    @Published private(set) var catalog:[AppleDraftKey]=[]
    @Published private(set) var recovered:[UUID:Payload]=[:]
    @Published private(set) var hosts:[ApplePortableHost]=[]
    @Published private(set) var projectionID=UUID()
    @Published private(set) var syncBusy=false
    private(set) var coordinator:Coordinator!
    private let root:URL
    private let identityProvider:@Sendable ()async throws->String
    private let defaults:UserDefaults
    private let consentKey:String
    private var identityTask:Task<String,Error>?
    private var deviceID:String?
    private var activityTask:Task<Void,Never>?
    private var activityID:UUID?
    private var syncTask:Task<Void,Never>?
    private var syncID:UUID?
    private var syncAgain=false
    private var active=false
    @Published private(set) var coreTransport:CrewPersonalCoreTransport?
    @Published private(set) var coreGrants:[CrewdRemoteGrant]=[]
    @Published private(set) var coreSynchronized=false
    private var coreBinding:CrewdPublicCommands.Binding?
    private weak var coreWorkspace:CrewdWorkspace?
    private var coreSyncedRevisions:[AppleDraftKey:UUID]=[:]
    private var coreSourcesKey:String{consentKey+".coreSources"}
    private var coreSelectionKey:String{consentKey+".coreSelection"}
    var usingCore:Bool{coordinator.current.id.core != nil}
    var coreStatus:CrewPersonalCoreWire.Status?{coreTransport?.status}
    private var projected:ApplePersonalNamespaceID = .local
    private var preferenceStore:NSUbiquitousKeyValueStore?
    private var preferenceObserver:NSObjectProtocol?
    private var defaultsObserver:NSObjectProtocol?
    private var preferenceScope:EditingScope?
    private var localAppearance:String?
    private var lastAppearance="system"
    private var applyingAppearance=false
    // Same actual key used by AppearanceMode and the existing Settings picker.
    private let appearanceKey="pendingcrew_appearance_mode"
    private var preferenceConsentKey:String{consentKey+".appearance"}
    var preferencesQualified:Bool{coordinator.configuration?.keyValueStoreIdentifier?.isEmpty == false}
    var preferencesOptedIn:Bool{defaults.bool(forKey:preferenceConsentKey)}
    func setPreferenceSync(_ enabled:Bool){
        defaults.set(enabled,forKey:preferenceConsentKey)
        updatePreferenceProjection()
    }
    var cloudConfigured:Bool{coordinator.configuration != nil}
    var cloudOptedIn:Bool{defaults.bool(forKey:consentKey)}
    var retainedNamespaces:[Namespace]{coordinator.retained.values.sorted{$0.id.stableName < $1.id.stableName}}
    private init(root:URL,deviceIdentity:(@Sendable ()async throws->String)?,configuration:ApplePersonalCloudConfiguration?,transportFactory:((ApplePersonalCloudConfiguration)throws->any AppleAccountTransport)?,defaults:UserDefaults){
        self.root=root;self.defaults=defaults
        consentKey="personal.cloud.optIn."+appleSyncDigest(Data(root.path.utf8))
        let service="com.pendingname.pendingcrew.personal-device."+appleSyncDigest(Data(root.path.utf8))
        identityProvider=deviceIdentity ?? {try await Task.detached{try Self.installationDeviceID(service:service)}.value}
        let local=makeNamespace(.local)
        coordinator=Coordinator(local:local,configuration:configuration,namespaceFactory:{[weak self] id in guard let self else{throw AppleSyncFailure.invalid};return self.makeNamespace(id)},transportFactory:transportFactory ?? {try AppleCloudKitTransport(containerIdentifier:$0.containerIdentifier,environment:$0.environment)})
        coordinator.didChange={[weak self] in self?.publishProjection()}
        if let data=defaults.data(forKey:coreSourcesKey),let sources=try? JSONDecoder().decode([PersonalCoreDraftProjection].self,from:data){
            for projection in sources.prefix(64){try? coordinator.retainCoreSource(.init(account:nil,core:projection,draftsCollection:"drafts",hostsCollection:"hosts"))}
        }
        if let bytes=defaults.data(forKey:coreSelectionKey),let projection=try? JSONDecoder().decode(PersonalCoreDraftProjection.self,from:bytes){
            try? coordinator.restoreCoreSelection(.init(account:nil,core:projection,draftsCollection:"drafts",hostsCollection:"hosts"))
        }
        publishProjection()
        defaultsObserver=NotificationCenter.default.addObserver(forName:UserDefaults.didChangeNotification,object:defaults,queue:.main){[weak self] _ in Task{@MainActor in self?.appearanceChanged()}}
    }
    private func installationID()async throws->String {
        if let identityTask{return try await identityTask.value}
        let provider=identityProvider;let task=Task{try await provider()};identityTask=task
        do{let value=try await task.value;deviceID=value;return value}catch{identityTask=nil;throw error}
    }
    private func makeNamespace(_ id:ApplePersonalNamespaceID)->Namespace {
        let directory=id.directory(in:root)
        var writer:CrewComposerDraftStore?
        var failure:String?
        do{try FileManager.default.createDirectory(at:directory,withIntermediateDirectories:true,attributes:[.posixPermissions:0o700]);writer = .durable(file:directory.appendingPathComponent("latest-input.json"),namespace:id.latestNamespace);failure=writer?.storageError}
        catch{failure="个人草稿目录不可写，当前原文仍保留；请导出后再退出。"}
        let latest=ApplePersonalLatestAccess<Payload>(readAll:{
            var values:[AppleDraftKey:Payload]=[:]
            for (storage,entry) in writer?.entries ?? [:]{
                guard let bytes=entry.sourceIdentity,let key=try? JSONDecoder().decode(AppleDraftKey.self,from:bytes),storage==Self.storageKey(key,namespace:id) else{continue}
                values[key]=entry.draft
            };return values
        },initializationError:failure,save:{key,payload in
            guard writer?.save(payload,for:Self.storageKey(key,namespace:id),crew:key.crewID,channel:Self.destinationLabel(key),retainEmpty:true,sourceIdentity:try? appleSyncEncode(key))==true else{return writer?.storageError ?? failure ?? "当前原文尚未保存，请导出。"};return nil
        },readRecovered:{
            var result:[UUID:Payload]=[:]
            let prefix=id.latestNamespace+"/retained/"
            for (storage,entry) in writer?.entries ?? [:] where storage.hasPrefix(prefix){
                guard let uuid=UUID(uuidString:String(storage.dropFirst(prefix.count))),let source=entry.sourceIdentity,(try? JSONDecoder().decode(AppleDraftKey.self,from:source)) != nil else{continue}
                result[uuid]=entry.draft
            };return result
        },saveRecovered:{uuid,key,payload in
            guard writer?.save(payload,for:id.latestNamespace+"/retained/"+uuid.uuidString.lowercased(),crew:key.crewID,channel:Self.destinationLabel(key),retainEmpty:true,sourceIdentity:try? appleSyncEncode(key))==true else{return writer?.storageError ?? failure ?? "保全原文尚未保存，请导出后退出。"};return nil
        })
        let owner=Namespace(id:id,latest:latest,stores:{[weak self] in
            guard let self else{throw AppleSyncFailure.invalid};let device:String
            if let core=id.core{device=core.writerID}else{device=try await self.installationID()}
            return .init(namespace:id,deviceID:device,drafts:try AppleBranchStore(root:directory,deviceID:device,account:id.account,collection:id.draftsCollection,coreProjection:id.core),hosts:try AppleBranchStore(root:directory,deviceID:device,account:id.account,collection:id.hostsCollection))
        })
        owner.didChange={[weak self] in self?.publishProjection()}
        owner.didCheckpoint={[weak self] in self?.scheduleSync()}
        return owner
    }
    private static func storageKey(_ key:AppleDraftKey,namespace:ApplePersonalNamespaceID)->String{namespace.latestNamespace+"/"+appleSyncDigest((try? appleSyncEncode(key)) ?? Data())}
    private static func destinationLabel(_ key:AppleDraftKey)->String{switch key.destination{case .channel(let v),.originalSession(let v):return v}}
    private func publishProjection(){
        guard let coordinator else{return};let current=coordinator.current
        if current.id != projected{projected=current.id;projectionID=UUID()}
        versions=current.versions.mapValues{$0.map{.init(id:$0.branchID,deviceID:$0.deviceID,own:$0.deviceID==(current.id.core?.writerID ?? deviceID),payload:$0.payload)}}
        errors=current.errors;unsavedLatest=current.unsavedLatest;saving=current.savingKeys
        catalog=current.catalog.sorted{Self.storageKey($0,namespace:current.id)<Self.storageKey($1,namespace:current.id)}
        recovered=current.recovered;hosts=current.hosts;initializationError=current.initializationError
        updatePreferenceProjection()
        if let core=current.id.core{
            if current.revisions != coreSyncedRevisions{coreSynchronized=false}
            if let adapter=coreTransport,adapter.ready(core){
                let issues=adapter.issues
                for (key,value) in issues where errors[key]==nil{errors[key]=value}
                status=syncBusy ? "正在与当前 Core 核对草稿版本":(coreSynchronized && issues.isEmpty ? "当前 Core 个人草稿版本已同步":"Core 个人草稿有待核对版本 · 本机原文已保留")
            }else{status="Core 草稿离线或资格待核对 · 原文仍在原空间本机保存"}
            return
        }
        switch coordinator.status {
        case .localOnly:status="草稿仅本机保存"
        case .notConfigured:status="个人同步暂未启用；草稿仅本机保存"
        case .discovering:status="正在核对个人 iCloud 账号"
        case .ready:status="个人 iCloud 已连接；等待同步"
        case .pending:status="个人草稿有待同步版本"
        case .synchronized:status="当前个人版本已同步"
        case .accountRefreshRequired:status="iCloud 账号已变化；原文保留，请核对账号"
        case .unavailable:status="个人同步暂不可用；原文仍保留"
        }
    }
    /// No KVS object is created without actual qualification + explicit consent
    /// + the current verified Apple account. No first-account preference seeding.
    private func updatePreferenceProjection(){
        let next=editingScope()
        guard preferencesQualified,preferencesOptedIn,cloudOptedIn,coordinator.optedIn,next.owner.id.account != nil,isCurrent(next) else{
            preferenceScope=nil
            if let observer=preferenceObserver{NotificationCenter.default.removeObserver(observer);preferenceObserver=nil}
            preferenceStore=nil
            if let original=localAppearance{applyAppearance(original);localAppearance=nil}
            return
        }
        if preferenceScope?.owner !== next.owner || preferenceScope?.projection != next.projection {
            if localAppearance == nil{localAppearance=defaults.string(forKey:appearanceKey) ?? "system";lastAppearance=localAppearance!}
            else if let original=localAppearance{applyAppearance(original)}
            preferenceScope=next
            if preferenceStore == nil {
                let store=NSUbiquitousKeyValueStore.default;preferenceStore=store
                preferenceObserver=NotificationCenter.default.addObserver(forName:NSUbiquitousKeyValueStore.didChangeExternallyNotification,object:store,queue:.main){[weak self] _ in Task{@MainActor in self?.receiveAppearance()}}
                store.synchronize()
            }
            receiveAppearance()
        }
    }
    private func preferenceKey(_ scope:EditingScope)->String{"personal.appearance."+scope.owner.id.stableName}
    private func receiveAppearance(){
        guard let scope=preferenceScope,isCurrent(scope),preferencesOptedIn,cloudOptedIn,
              let value=preferenceStore?.string(forKey:preferenceKey(scope)),["system","light","dark"].contains(value) else{return}
        applyAppearance(value)
    }
    private func applyAppearance(_ value:String){
        applyingAppearance=true;lastAppearance=value;defaults.set(value,forKey:appearanceKey);applyingAppearance=false
    }
    private func appearanceChanged(){
        let value=defaults.string(forKey:appearanceKey) ?? "system"
        guard !applyingAppearance,value != lastAppearance else{return};lastAppearance=value
        guard let scope=preferenceScope,isCurrent(scope),preferencesOptedIn,cloudOptedIn,["system","light","dark"].contains(value) else{return}
        preferenceStore?.set(value,forKey:preferenceKey(scope))
    }
    private func setActive(_ value:Bool){
        active=value
        if !value{activityTask?.cancel();activityTask=nil;activityID=nil;return}
        if usingCore{scheduleSync();return}
        guard cloudConfigured,cloudOptedIn,activityTask==nil else{return}
        let operation=UUID();activityID=operation
        activityTask=Task{[weak self] in guard let self else{return};defer{if self.activityID==operation{self.activityTask=nil;self.activityID=nil}};do{try await self.coordinator.enable();guard self.activityID==operation,!Task.isCancelled,self.active else{return};self.publishProjection();self.scheduleSync()}catch{if self.activityID==operation{self.publishProjection()}}}
    }
    func enablePersonalSync()async throws{
        guard cloudConfigured else{throw AppleSyncFailure.invalid}
        defaults.removeObject(forKey:coreSelectionKey)
        defaults.set(true,forKey:consentKey)
        try await coordinator.enable();publishProjection();scheduleSync()
    }
    func disablePersonalSync()async{
        defaults.removeObject(forKey:coreSelectionKey)
        defaults.set(false,forKey:consentKey);activityTask?.cancel();activityTask=nil;activityID=nil;syncTask?.cancel();syncTask=nil;syncID=nil;syncAgain=false;syncBusy=false
        await coordinator.disable();publishProjection()
    }
    private func scheduleSync(){
        guard active,(usingCore && coreTransport != nil) || (cloudOptedIn && coordinator.optedIn && coordinator.current.id.account != nil) else{return}
        if syncTask != nil{syncAgain=true;return}
        let operation=UUID();syncID=operation
        syncTask=Task{[weak self] in guard let self else{return};self.syncBusy=true;defer{if self.syncID==operation{self.syncBusy=false;self.syncTask=nil;self.syncID=nil}}
            repeat{self.syncAgain=false;do{if self.usingCore{guard let adapter=self.coreTransport else{throw AppleSyncFailure.accountChanged};self.coreSynchronized = try await self.coordinator.current.synchronizeCore(using:adapter);if self.coreSynchronized{self.coreSyncedRevisions=self.coordinator.current.revisions}}else{try await self.coordinator.synchronize()}}catch{if self.syncID==operation{self.publishProjection()};return};guard self.syncID==operation else{return};self.publishProjection()}while self.syncAgain && !Task.isCancelled && self.active && (self.usingCore || self.cloudOptedIn)
        }
    }
    func synchronizePersonal()async throws{
        if usingCore{guard let coreTransport else{throw AppleSyncFailure.accountChanged};coreSynchronized = try await coordinator.current.synchronizeCore(using:coreTransport);if coreSynchronized{coreSyncedRevisions=coordinator.current.revisions}}else{try await coordinator.synchronize()};publishProjection()
    }
    var cacheSuffix:String{coordinator.current.id == .local ? "":"/personal-"+coordinator.current.id.stableName}
    func editingScope()->EditingScope{.init(owner:coordinator.current,projection:projectionID)}
    func isCurrent(_ scope:EditingScope)->Bool{scope.owner === coordinator.current && scope.projection==projectionID && scope.owner.isSelected}
    func localDraft(_ key:AppleDraftKey)->Payload?{coordinator.current.localDraft(key)}
    func checkpoint(_ payload:Payload,key:AppleDraftKey,scope:EditingScope?=nil){
        let owner=scope?.owner ?? coordinator.current
        if owner === coordinator.current,owner.id.core != nil{coreSynchronized=false}
        if let scope,!isCurrent(scope){
            if owner.localDraft(key)==payload{return}
            owner.retainStale(payload,key:key,id:Self.recoveryID(scope,key:key));publishProjection();return
        }
        do{_ = try owner.checkpoint(payload,key:key)}catch{checkpoint(payload,key:key,scope:scope ?? editingScope());return}
        publishProjection()
    }
    private static func recoveryID(_ scope:EditingScope,key:AppleDraftKey)->UUID{
        let digest=appleSyncDigest(Data((scope.editorID.uuidString+"/"+Self.storageKey(key,namespace:scope.owner.id)).utf8))
        let chars=Array(digest.prefix(32))
        return UUID(uuidString:String(chars[0..<8])+"-"+String(chars[8..<12])+"-"+String(chars[12..<16])+"-"+String(chars[16..<20])+"-"+String(chars[20..<32]))!
    }
    func staleExportTicket(_ key:AppleDraftKey,payload:Payload,scope:EditingScope?)->RecoveredExportTicket?{
        guard let scope,!isCurrent(scope),let value=recoveredExportTicket(Self.recoveryID(scope,key:key),owner:scope.owner),value.value.payload==payload else{return nil};return value
    }
    func refresh(_ key:AppleDraftKey)async{do{try await coordinator.current.refresh()}catch{};publishProjection()}
    func restoreOwn(_ key:AppleDraftKey)async->Payload?{let scope=editingScope();if let v=scope.owner.localDraft(key){return v};await refresh(key);guard isCurrent(scope) else{return nil};return scope.owner.localDraft(key) ?? versions[key]?.first(where:{$0.own})?.payload}
    func snapshot(_ key:AppleDraftKey,payload:Payload,scope:EditingScope?=nil)->Snapshot?{if let scope,!isCurrent(scope){return nil};let owner=coordinator.current;guard let value=owner.snapshot(key:key,payload:payload,empty:.init()) else{return nil};return .init(owner:owner,value:value)}
    func isCurrent(_ value:Snapshot)->Bool{coordinator.current === value.owner && value.owner.isCurrent(value.value)}
    @discardableResult func clearIfUnchanged(_ value:Snapshot)->Bool{guard isCurrent(value) else{return false};do{_ = try value.owner.clearIfUnchanged(value.value,empty:.init());publishProjection();return true}catch{return false}}
    func resolve(_ key:AppleDraftKey,payload:Payload?,retiring:Set<UUID>,expected:Snapshot)async throws->Snapshot{
        guard isCurrent(expected) else{throw AppleSyncFailure.accountChanged};coreSynchronized=false;let ticket=try await expected.owner.resolve(key:key,payload:payload,empty:.init(),retiring:retiring,expected:expected.value,coreTransport:coreTransport);let result=Snapshot(owner:expected.owner,value:ticket);guard isCurrent(result) else{throw AppleSyncFailure.accountChanged};publishProjection();scheduleSync();return result
    }
    func saveCurrentAsNewVersion(_ key:AppleDraftKey,payload:Payload,expected:Snapshot)async throws{
        guard isCurrent(expected),expected.key==key else{throw AppleSyncFailure.accountChanged}
        coreSynchronized=false;try await expected.owner.saveNewVersion(payload,key:key);guard coordinator.current === expected.owner else{throw AppleSyncFailure.accountChanged};publishProjection();scheduleSync()
    }
    /// Selecting a Core changes no Apple account and performs no implicit import.
    /// Rebinding after a connection change does not switch the selected namespace.
    func bindCore(_ workspace:CrewdWorkspace?){
        let binding=workspace.flatMap{try? $0.publicCommandBinding(for:$0.client)}
        if coreWorkspace === workspace{
            if coreBinding != binding{coreBinding=binding;coreSynchronized=false;publishProjection();if usingCore,binding != nil{scheduleSync()}}
            return
        }
        coreBinding=binding
        coreTransport?.invalidate();coreWorkspace=workspace;coreGrants=[];coreSynchronized=false
        guard let workspace else{coreTransport=nil;publishProjection();return}
        let adapter=CrewPersonalCoreTransport(.init(origin:{[weak workspace,weak self] in
            guard let self,let workspace,self.coreWorkspace === workspace,workspace.client.readyForWork else{return nil};return try? workspace.publicCommandBinding(for:workspace.client)
        },selected:{[weak self] projection in self?.coordinator.current.id.core==projection && self?.coordinator.current.isSelected==true},rpc:{[weak workspace] method,params in guard let workspace else{throw AppleSyncFailure.accountChanged};let epoch=workspace.client.connectionEpoch;let result=try await workspace.client.call(method,params)
            if method=="personalDraft.status"{guard workspace.client.connectionEpoch==epoch else{throw CancellationError()};_ = try CrewPersonalCoreWire.Status(result);try await workspace.client.subscribePersonalDraftEvents()};return result},submit:{[weak workspace] operation,params,crew,writer in
            guard let workspace else{throw AppleSyncFailure.accountChanged};return try await workspace.submitPublicCommand(operation,params:params,crew:crew,validateReceipt:{receipt in if receipt.phase == .committed{try CrewPersonalCoreWire.validateResult(receipt.value["result"],command:.object(["operation":.string(operation.rawValue),"params":params]),expectedWriter:writer)}})
        },history:{[weak workspace] in guard let workspace else{throw AppleSyncFailure.accountChanged};return try workspace.personalDraftHistory()},unknown:{[weak workspace] in Set(workspace?.durableUnknown.map(\.id) ?? [])},lookup:{[weak workspace] row in guard let workspace else{throw AppleSyncFailure.accountChanged};return try await workspace.lookupOriginalCommand(row)},acknowledge:{[weak workspace] row,receipt in guard let workspace else{throw AppleSyncFailure.accountChanged};try workspace.acknowledgePersonalDraft(row,receipt:receipt)},fence:{[weak workspace] row in guard let workspace else{throw AppleSyncFailure.accountChanged};return try await workspace.fencePersonalDraft(row)}))
        adapter.changed={[weak self,weak adapter] in guard self?.coreTransport === adapter else{return};self?.publishProjection()};coreTransport=adapter
        publishProjection();if usingCore{scheduleSync()}
    }
    func coreChanged(_ workspace:CrewdWorkspace){guard coreWorkspace === workspace,usingCore else{return};coreSynchronized=false;scheduleSync()}
    func inspectCore()async throws{guard let coreTransport else{throw AppleSyncFailure.accountChanged};_ = try await coreTransport.refreshStatus();publishProjection()}
    func selectCore()async throws{
        guard let adapter=coreTransport else{throw AppleSyncFailure.accountChanged}
        let original=projectionID
        var current=try await adapter.refreshStatus()
        guard coreTransport === adapter,projectionID==original,current.member?.revoked==false else{throw AppleSyncFailure.accountChanged}
        if current.member?.optedIn != true{try await adapter.consent(true);guard let updated=adapter.status else{throw AppleSyncFailure.invalid};current=updated}
        guard coreTransport === adapter,projectionID==original,current.available,let projection=current.projection else{throw AppleSyncFailure.accountChanged}
        var sources=defaults.data(forKey:coreSourcesKey).flatMap{try? JSONDecoder().decode([PersonalCoreDraftProjection].self,from:$0)} ?? []
        if !sources.contains(projection){guard sources.count<64 else{throw CrewdClientError(message:"已保留的 Core 来源达到上限；原文未删除，请先导出整理。",kind:.notSubmitted)};sources.append(projection)}
        stopSync();defaults.set(false,forKey:consentKey)
        try await coordinator.selectCore(.init(account:nil,core:projection,draftsCollection:"drafts",hostsCollection:"hosts"))
        guard coordinator.current.id.core==projection,coreTransport === adapter else{throw AppleSyncFailure.accountChanged}
        defaults.set(try appleSyncEncode(sources),forKey:coreSourcesKey)
        defaults.set(try appleSyncEncode(projection),forKey:coreSelectionKey);publishProjection();scheduleSync()
    }
    func disableCoreConsent()async throws{
        guard let adapter=coreTransport else{throw AppleSyncFailure.accountChanged}
        stopSync();coreSynchronized=false
        try await adapter.consent(false);guard coreTransport === adapter else{throw AppleSyncFailure.accountChanged};publishProjection()
    }
    private func stopSync(){syncTask?.cancel();syncTask=nil;syncID=nil;syncAgain=false;syncBusy=false;activityTask?.cancel();activityTask=nil;activityID=nil}
    func loadCoreMembers()async throws{guard let adapter=coreTransport else{throw AppleSyncFailure.accountChanged};try await adapter.loadMembers();guard let workspace=coreWorkspace,coreTransport === adapter,adapter.status?.owner==true else{throw AppleSyncFailure.accountChanged};let binding=try workspace.publicCommandBinding(for:workspace.client);let values=try await workspace.client.call("remote.grants",.null);guard coreWorkspace === workspace,coreTransport === adapter,try workspace.publicCommandBinding(for:workspace.client)==binding else{throw AppleSyncFailure.accountChanged};coreGrants=try JSONDecoder().decode([CrewdRemoteGrant].self,from:JSONEncoder().encode(values));publishProjection()}
    func retry(_ key:AppleDraftKey){if let payload=localDraft(key){checkpoint(payload,key:key)}}
    func exportTicket(_ key:AppleDraftKey,payload:Payload,scope:EditingScope?=nil)->ExportTicket?{let owner=scope?.owner ?? coordinator.current;guard owner.localDraft(key)==payload,let ticket=owner.retainedTicket(key:key) else{return nil};return .init(owner:owner,value:ticket)}
    func copyRetained(_ ticket:ExportTicket,replacing target:Snapshot?)throws {
        var replacing:[AppleDraftKey:Namespace.Ticket]=[:]
        if let target{guard isCurrent(target) else{throw AppleSyncFailure.invalid};replacing[target.key]=target.value}
        try coordinator.copySelected([ticket.value],from:ticket.owner,replacing:replacing)
        publishProjection();scheduleSync()
    }
    func retainedExportTicket(_ owner:Namespace,key:AppleDraftKey)->ExportTicket? {
        guard coordinator.retained[owner.id] === owner,let value=owner.retainedTicket(key:key) else{return nil};return .init(owner:owner,value:value)
    }
    func copyHost(_ ticket:Namespace.HostTicket,from source:Namespace)async throws{try await coordinator.copySelectedHosts([ticket],from:source);publishProjection();scheduleSync()}
    func acknowledgeExport(_ value:ExportTicket){try? value.owner.acknowledgeExport(value.value);publishProjection()}
    func recoveredExportTicket(_ id:UUID,owner:Namespace?=nil)->RecoveredExportTicket?{let owner=owner ?? coordinator.current;guard coordinator.retained[owner.id] === owner,let value=owner.recoveredTicket(id) else{return nil};return .init(owner:owner,value:value)}
    func acknowledgeRecoveredExport(_ ticket:RecoveredExportTicket){try? ticket.owner.acknowledgeRecovered(ticket.value);publishProjection()}
    func loadCatalog()async{do{try await coordinator.current.refresh()}catch{};publishProjection()}
    func loadHosts()async{await loadCatalog()}
    func export(_ key:AppleDraftKey)throws->Data{guard let payload=localDraft(key) else{throw AppleSyncFailure.invalid};return try appleSyncEncode(payload)}
    func rememberHost(_ host:ApplePortableHost)async{do{try await coordinator.current.rememberHost(host);publishProjection();scheduleSync()}catch{initializationError="主机目录未保存，原设备配对未改变。"}}
    nonisolated private static func installationDeviceID(service:String)throws->String {
        let query:[String:Any]=[kSecClass as String:kSecClassGenericPassword,kSecAttrService as String:service,kSecAttrAccount as String:"installation",kSecAttrSynchronizable as String:false]
        var lookup=query;lookup[kSecReturnData as String]=true;lookup[kSecMatchLimit as String]=kSecMatchLimitOne
        var value:CFTypeRef?
        let read=SecItemCopyMatching(lookup as CFDictionary,&value)
        if read == errSecSuccess,let data=value as? Data,let id=String(data:data,encoding:.utf8),UUID(uuidString:id) != nil{return id}
        guard read == errSecItemNotFound else{throw AppleSyncFailure.storage}
        let id=UUID().uuidString.lowercased();var insert=query
        insert[kSecValueData as String]=Data(id.utf8)
        insert[kSecAttrAccessible as String]=kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let result=SecItemAdd(insert as CFDictionary,nil)
        if result == errSecDuplicateItem{return try installationDeviceID(service:service)}
        guard result == errSecSuccess else{throw AppleSyncFailure.storage}
        return id
    }
}
