import Foundation
import Combine
import Security
import Darwin

/// One original-App owner, shared by every window. This layer stores personal
/// input only; it has no Core writer, session runner, enrollment or replay API.
@MainActor final class CrewPersonalDraftService:ObservableObject {
    typealias Payload=CrewComposerDraftStore.Draft
    typealias Store=AppleBranchStore<Payload>
    struct Version:Identifiable {
        let id:UUID
        let deviceID:String
        let own:Bool
        let payload:Payload
    }
    private static var instance:CrewPersonalDraftService?
    private static var owners:[String:CrewPersonalDraftService]=[:]
    static var shared:CrewPersonalDraftService{
        if let instance{return instance}
        let next=owned(root:PendingCrewDataRoot.url.appendingPathComponent("personal-input",isDirectory:true))
        instance=next;return next
    }
    static var hasUnsavedInProcess:Bool{owners.values.contains{!$0.unsavedLatest.isEmpty}}
    static var unsavedExitToken:String? {
        let tokens=owners.sorted{$0.key < $1.key}.flatMap{path,owner in
            owner.unsavedLatest.compactMap{key in owner.revisions[key].map{path+owner.storageKey(key)+$0.uuidString}}.sorted()
        }
        return tokens.isEmpty ? nil:appleSyncDigest(Data(tokens.joined(separator:"\n").utf8))
    }
    static func owned(root:URL,deviceIdentity:(@Sendable ()async throws->String)?=nil)->CrewPersonalDraftService {
        var canonical=root
        do{
            try FileManager.default.createDirectory(at:root,withIntermediateDirectories:true,attributes:[.posixPermissions:0o700])
            if let value=realpath(root.path,nil){canonical=URL(fileURLWithPath:String(cString:value),isDirectory:true);free(value)}
        }catch{}
        if let value=owners[canonical.path]{return value}
        let value=CrewPersonalDraftService(root:canonical,deviceIdentity:deviceIdentity)
        owners[canonical.path]=value;return value
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
    private let root:URL
    private let identityProvider:@Sendable ()async throws->String
    private var latest:CrewComposerDraftStore?
    private var storeTask:Task<(Store,String),Error>?
    private var hostStore:AppleBranchStore<ApplePortableHost>?
    private var writers:[AppleDraftKey:Task<Void,Never>]=[:]
    private var revisions:[AppleDraftKey:UUID]=[:]
    private var latestMemory:[AppleDraftKey:Payload]=[:]
    private var deviceID:String?
    // No CloudKit adapter or ubiquitous KVS is constructed without qualified
    // actual container/signing configuration and a separate explicit opt-in.
    let cloudConfigured=false
    private init(root:URL,deviceIdentity:(@Sendable ()async throws->String)?){
        self.root=root
        let service="com.pendingname.pendingcrew.personal-device."+appleSyncDigest(Data(root.path.utf8))
        identityProvider=deviceIdentity ?? {try await Task.detached{try Self.installationDeviceID(service:service)}.value}
        do{
            try FileManager.default.createDirectory(at:root,withIntermediateDirectories:true,attributes:[.posixPermissions:0o700])
            let file=root.appendingPathComponent("latest-input.json")
            latest=CrewComposerDraftStore.durable(file:file,namespace:"personal")
            if let error=latest?.storageError{initializationError=error}
        }catch{initializationError="个人草稿目录不可写，当前原文仍保留；请导出后再退出。"}
    }
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
    private func store()async throws->(Store,String){
        if let storeTask{return try await storeTask.value}
        let root=self.root,identityProvider=self.identityProvider
        let task=Task<(Store,String),Error>{
            let id=try await identityProvider()
            return (try Store(root:root,deviceID:id,account:nil,collection:"drafts"),id)
        }
        storeTask=task
        do{let result=try await task.value;deviceID=result.1;return result}
        catch{storeTask=nil;throw error}
    }
    private func storageKey(_ key:AppleDraftKey)->String{
        "personal/"+appleSyncDigest((try? appleSyncEncode(key)) ?? Data())
    }
    func localDraft(_ key:AppleDraftKey)->Payload?{
        if let current=latestMemory[key]{return current}
        return latest?.entries[storageKey(key)]?.draft
    }
    /// Synchronous original draft persistence protects the latest whole payload
    /// before the asynchronous branch checkpoint; a failed checkpoint never
    /// claims durable success or drops the original latest input.
    func checkpoint(_ payload:Payload,key:AppleDraftKey){
        latestMemory[key]=payload
        let revision=UUID();revisions[key]=revision
        if latest?.save(payload,for:storageKey(key),crew:key.crewID,channel:destinationLabel(key),retainEmpty:true) != true {
            unsavedLatest.insert(key);errors[key]=latest?.storageError ?? initializationError ?? "当前原文尚未保存，请导出。"
        }else{unsavedLatest.remove(key)}
        let previous=writers[key];previous?.cancel();saving.insert(key)
        writers[key]=Task{[weak self] in
            try? await Task.sleep(for:.milliseconds(250))
            guard !Task.isCancelled else{return}
            await previous?.value
            guard let self,!Task.isCancelled,self.revisions[key] == revision else{return}
            do{
                let (store,_)=try await self.store()
                _ = try await store.checkpoint(key:key,payload:payload)
                await self.refresh(key)
                if self.revisions[key] == revision {
                    self.unsavedLatest.remove(key)
                    self.errors[key]=self.latest?.storageError == nil ? nil:"版本已保存在本机；快捷草稿保存失败，可重试。"
                }
            }catch{if self.revisions[key] == revision{self.errors[key]=self.unsavedLatest.contains(key) ? "原文未能保存到本机，请导出后再退出。":"最新原文已保存在本机；版本保存失败，可重试或导出。"}}
            if self.revisions[key] == revision{self.writers[key]=nil;self.saving.remove(key)}
        }
    }
    func refresh(_ key:AppleDraftKey)async{
        do{
            let (store,id)=try await store()
            let values=await store.active(key:key)
            versions[key]=values.map{.init(id:$0.branchID,deviceID:$0.deviceID,own:$0.deviceID == id,payload:$0.payload)}
            if let error=await store.storageError{errors[key]="草稿版本尚未保存："+error}
        }catch{errors[key]="本机版本暂不可读，原文保留；可重试或导出。"}
    }
    func restoreOwn(_ key:AppleDraftKey)async->Payload?{
        if let latest=localDraft(key){return latest}
        await refresh(key)
        return localDraft(key) ?? versions[key]?.first(where:{$0.own})?.payload
    }
    /// Caller explicitly confirms which branches are retired. Never merge on
    /// arrival or replace a nonempty composer just because another device syncs.
    func resolve(_ key:AppleDraftKey,payload:Payload?,retiring:Set<UUID>)async throws{
        if let previous=writers[key]{await previous.value}
        let (store,_)=try await store()
        let replacement=payload ?? Payload()
        latestMemory[key]=replacement;revisions[key]=UUID()
        let latestSaved=latest?.save(replacement,for:storageKey(key),crew:key.crewID,channel:destinationLabel(key),retainEmpty:true) == true
        if !latestSaved{unsavedLatest.insert(key);errors[key]=latest?.storageError ?? "合并原文尚未保存，请导出。"}
        do{_ = try await store.resolve(key:key,payload:payload,retiring:retiring)}
        catch{errors[key]=latestSaved ? "合并原文已保存在本机；版本保存失败，尚未完成分支合并。":"合并原文尚未保存，请导出后再退出。";throw error}
        if latestSaved{unsavedLatest.remove(key);errors[key]=nil}
        else{throw AppleSyncFailure.storage}
        await refresh(key)
    }
    func retry(_ key:AppleDraftKey){if let payload=localDraft(key){checkpoint(payload,key:key)}}
    struct ExportTicket {let key:AppleDraftKey;let revision:UUID;let payload:Payload}
    func exportTicket(_ key:AppleDraftKey,payload:Payload)->ExportTicket?{
        guard latestMemory[key] == payload,let revision=revisions[key] else{return nil}
        return .init(key:key,revision:revision,payload:payload)
    }
    func acknowledgeExport(_ ticket:ExportTicket){
        guard revisions[ticket.key] == ticket.revision,latestMemory[ticket.key] == ticket.payload else{return}
        unsavedLatest.remove(ticket.key)
        errors[ticket.key]="当前原文已导出；本机保存仍须重试。"
    }
    func loadCatalog()async{
        do{
            let (store,_)=try await store()
            catalog=await store.keys().sorted{storageKey($0) < storageKey($1)}
            recovered=await store.recoveredOriginals()
            if await store.storageError != nil{initializationError="草稿版本文件不可读，原文件已保留；当前输入可另行导出。"}
        }catch{initializationError="本机草稿目录暂不可读；原文件未覆盖。"}
    }
    func loadHosts()async{
        do{
            let (_,id)=try await store()
            if hostStore == nil{hostStore=try AppleBranchStore<ApplePortableHost>(root:root,deviceID:id,account:nil,collection:"hosts")}
            guard let hostStore else{return}
            var values:[ApplePortableHost]=[]
            for key in await hostStore.keys(){for version in await hostStore.active(key:key){if !values.contains(version.payload){values.append(version.payload)}}}
            hosts=values.sorted{$0.displayName < $1.displayName}
            if await hostStore.storageError != nil{initializationError="主机目录文件暂不可读，原文件未覆盖。"}
        }catch{initializationError="个人主机目录暂不可读；本设备原配对不受影响。"}
    }
    func export(_ key:AppleDraftKey)throws->Data{
        guard let payload=localDraft(key) else{throw AppleSyncFailure.invalid}
        return try appleSyncEncode(payload)
    }
    private func destinationLabel(_ key:AppleDraftKey)->String{
        switch key.destination{case .channel(let value),.originalSession(let value):return value}
    }
    /// Directory entries are discovery hints only; this method does not connect
    /// or copy the saved pair/credential. TLS host:port must still be enrolled.
    func rememberHost(_ host:ApplePortableHost)async{
        do{
            let (_,id)=try await store()
            if hostStore == nil{hostStore=try AppleBranchStore<ApplePortableHost>(root:root,deviceID:id,account:nil,collection:"hosts")}
            let key=AppleDraftKey(authorityID:host.transportAuthorityID,databaseID:host.databaseID,crewID:"directory",destination:.originalSession(host.verifiedSPKI),kind:"host")
            _ = try await hostStore?.checkpoint(key:key,payload:host)
            if !hosts.contains(host){hosts.append(host)}
        }catch{initializationError="主机目录未保存，原设备配对未改变。"}
    }
}
