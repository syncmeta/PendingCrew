import Foundation
import CryptoKit
import Darwin

/// Durable frontend observations, never an authority receipt or replay queue.
@MainActor
final class CrewdPresentationJournal {
    struct Scope: Codable, Equatable {
        let namespace: UUID
        let localDirectory: String?
        let authorityID: String?
        let databaseID: String?
        let grantID: String?
        let grantRevision: String?
        var authenticatedDatabaseKnown: Bool { authorityID != nil && databaseID != nil }
    }
    struct Prepared: Codable, Identifiable {
        let format: Int
        let id: UUID // local observation identity only
        let scope: Scope
        let method: String
        let params: CrewdValue
        let crew: String
        let preparedAt: Date
    }
    struct Outcome: Codable {
        let format: Int
        let observationID: UUID
        let preparedSHA256: String
        let state: String // replyObserved/refused/unknown; not executed/consumed
        let observedAt: Date
        let detail: String?
    }
    let scope: Scope
    func composerDraftFile(namespace: String) -> URL {
        directory.appendingPathComponent("composer-drafts-" + Self.hash(Data(namespace.utf8)) + ".json")
    }
    private let directory: URL
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder
    private enum StorageError: LocalizedError {
        case refused(String)
        var errorDescription: String? { if case .refused(let reason) = self { return reason }; return nil }
    }
    init(root: URL, scope: Scope, readOnly:Bool=false) throws {
        self.scope = scope
        encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        decoder = JSONDecoder(); decoder.dateDecodingStrategy = .iso8601
        if readOnly{guard FileManager.default.fileExists(atPath:root.path) else{throw CrewdPublicWire.invalid()}}
        try Self.privateDirectory(root)
        directory = root.appendingPathComponent(scope.namespace.uuidString.lowercased())
        if readOnly{
            guard FileManager.default.fileExists(atPath:directory.path) else{throw CrewdPublicWire.invalid()}
            try Self.privateDirectory(directory)
        }else{
            try Self.privateDirectory(directory)
            for name in ["prepared","outcomes","reconciled","document-stage-inputs","document-stage-attempts","document-stage-results","document-stage-finished","codex-input-results","codex-input-originals","codex-input-choices"]{try Self.privateDirectory(directory.appendingPathComponent(name))}
        }
    }
    static func localNamespace(directory: URL) throws -> UUID {
        let canonical = directory.standardizedFileURL.resolvingSymlinksInPath()
        guard canonical.path == directory.standardizedFileURL.path else { throw StorageError.refused("前端命名空间拒绝别名或符号链接目录。") }
        if let old = ProcessInfo.processInfo.environment["PENDINGCREW_LEGACY_DATA_DIR"] {
            let legacy = URL(fileURLWithPath: old).standardizedFileURL.resolvingSymlinksInPath().path
            guard canonical.path != legacy, !canonical.path.hasPrefix(legacy + "/") else { throw StorageError.refused("候选前端不能写入旧版日常数据目录。") }
        }
        try privateDirectory(canonical)
        let front = canonical.appendingPathComponent("frontend-presentation")
        try privateDirectory(front)
        let file = front.appendingPathComponent("namespace.json")
        if FileManager.default.fileExists(atPath: file.path) {
            try privateFile(file)
            guard let value = UUID(uuidString: String(decoding: try Data(contentsOf: file), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)) else { throw StorageError.refused("前端命名空间记录损坏；未覆盖原记录。") }
            return value
        }
        let id = UUID()
        do { try immutableWrite(Data((id.uuidString.lowercased() + "\n").utf8), to: file) }
        catch {
            // Another GUI may have created the immutable namespace meanwhile.
            if FileManager.default.fileExists(atPath: file.path) { return try localNamespace(directory: directory) }
            throw error
        }
        return id
    }
    /// Prepare before any original frontend singleton can create a cache directory.
    /// This is presentation storage, separate from both crewd and unknown observations.
    static func prepareUICache(directory: URL) throws -> URL {
        _ = try localNamespace(directory: directory)
        let cache = directory.appendingPathComponent("frontend-presentation/ui-cache")
        try privateDirectory(cache)
        return cache
    }
    private static func privateDirectory(_ directory: URL) throws {
        let fm = FileManager.default
        if !fm.fileExists(atPath: directory.path) {
            try fm.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        }
        let attrs = try fm.attributesOfItem(atPath: directory.path)
        guard attrs[.type] as? FileAttributeType == .typeDirectory,
              (attrs[.ownerAccountID] as? NSNumber)?.uint32Value == geteuid(),
              (attrs[.posixPermissions] as? NSNumber)?.intValue == 0o700,
              directory.standardizedFileURL.path == directory.standardizedFileURL.resolvingSymlinksInPath().path else {
            throw StorageError.refused("前端历史目录须是当前用户的独立 0700 实体目录。")
        }
    }
    private static func privateFile(_ file: URL) throws {
        let attrs = try FileManager.default.attributesOfItem(atPath: file.path)
        guard attrs[.type] as? FileAttributeType == .typeRegular,
              (attrs[.ownerAccountID] as? NSNumber)?.uint32Value == geteuid(),
              (attrs[.posixPermissions] as? NSNumber)?.intValue == 0o600 else {
            throw StorageError.refused("前端历史文件须是当前用户的 0600 普通文件。")
        }
    }
    private static func immutableWrite(_ bytes: Data, to file: URL) throws {
        let fd = open(file.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, mode_t(0o600))
        guard fd >= 0 else { throw StorageError.refused("无法创建独立前端历史记录；未提交后台操作。") }
        defer { close(fd) }
        try bytes.withUnsafeBytes { buffer in
            var offset = 0
            while offset < buffer.count {
                let written = Darwin.write(fd, buffer.baseAddress!.advanced(by: offset), buffer.count - offset)
                if written < 0 && errno == EINTR { continue }
                guard written > 0 else { throw StorageError.refused("前端历史未完整写入；保留原文件，不提交后台操作。") }
                offset += written
            }
        }
        guard fsync(fd) == 0 else { throw StorageError.refused("前端历史无法同步到磁盘；未提交后台操作。") }
        let parent = open(file.deletingLastPathComponent().path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
        guard parent >= 0 else { throw StorageError.refused("前端历史目录同步失败；未提交后台操作。") }
        defer { close(parent) }
        guard fsync(parent) == 0 else { throw StorageError.refused("前端历史目录同步失败；未提交后台操作。") }
    }
    nonisolated private static func hash(_ bytes: Data) -> String { CryptoKit.SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined() }
    func prepare(method: String, params: CrewdValue, crew: String) throws -> Prepared {
        let record = Prepared(format: 1, id: UUID(), scope: scope, method: method, params: params, crew: crew, preparedAt: Date())
        try Self.immutableWrite(encoder.encode(record), to: directory.appendingPathComponent("prepared").appendingPathComponent(record.id.uuidString.lowercased() + ".json"))
        return record
    }
    func finish(_ record: Prepared, state: String, detail: String? = nil) throws {
        guard record.scope == scope, ["replyObserved", "refused", "unknown"].contains(state) else { throw StorageError.refused("前端历史作用域或状态不符。") }
        let prepared = directory.appendingPathComponent("prepared").appendingPathComponent(record.id.uuidString.lowercased() + ".json")
        try Self.privateFile(prepared)
        let result = Outcome(format: 1, observationID: record.id, preparedSHA256: Self.hash(try Data(contentsOf: prepared)), state: state, observedAt: Date(), detail: detail)
        try Self.immutableWrite(encoder.encode(result), to: directory.appendingPathComponent("outcomes").appendingPathComponent(record.id.uuidString.lowercased() + ".json"))
    }
    /// Append a verified observation without rewriting the immutable Unknown.
    func acknowledge(_ record: Prepared, detail: String) throws {
        guard record.scope == scope else { throw StorageError.refused("核对作用域不符。") }
        let name = record.id.uuidString.lowercased() + ".json"
        let prepared = directory.appendingPathComponent("prepared").appendingPathComponent(name)
        try Self.privateFile(prepared)
        let bytes = try Data(contentsOf: prepared)
        let saved = try decoder.decode(Prepared.self, from: bytes)
        guard saved.id == record.id, saved.scope == record.scope, saved.params == record.params,
              saved.method == record.method, saved.crew == record.crew else { throw StorageError.refused("核对原记录不符。") }
        let hash = Self.hash(bytes)
        let destination = directory.appendingPathComponent("reconciled").appendingPathComponent(name)
        if FileManager.default.fileExists(atPath: destination.path) {
            try Self.privateFile(destination)
            let old = try decoder.decode(Outcome.self, from: Data(contentsOf: destination))
            guard old.format == 1, old.observationID == record.id, old.preparedSHA256 == hash, old.state == "replyObserved" else { throw StorageError.refused("核对观察与原记录不符。") }
            return
        }
        let result = Outcome(format: 1, observationID: record.id, preparedSHA256: hash, state: "replyObserved", observedAt: Date(), detail: detail)
        try Self.immutableWrite(encoder.encode(result), to: destination)
    }
    struct DocumentStage:Codable,Equatable,Identifiable {
        let id:UUID
        let scope:Scope
        let crew:String
        let channel:String
        let title:String
        let body:String
        let context:CrewdValue
    }
    private func stageFile(_ area:String,_ id:UUID)->URL{directory.appendingPathComponent("document-stage-"+area).appendingPathComponent(id.uuidString.lowercased()+".json")}
    private func checkStage(_ stage:DocumentStage)throws {
        guard stage.scope == scope else{throw StorageError.refused("文档保存阶段的来源身份不符。")}
        let file=stageFile("inputs",stage.id);try Self.privateFile(file)
        guard try decoder.decode(DocumentStage.self,from:Data(contentsOf:file)) == stage else{throw StorageError.refused("原文档保存阶段不符。")}
    }
    func currentDocumentStage(crew:String,channel:String)throws->DocumentStage? {
        var stages:[DocumentStage]=[]
        for file in try FileManager.default.contentsOfDirectory(at:directory.appendingPathComponent("document-stage-inputs"),includingPropertiesForKeys:nil){
            try Self.privateFile(file)
            let stage=try decoder.decode(DocumentStage.self,from:Data(contentsOf:file));guard stage.scope == scope else{continue};try checkStage(stage)
            if stage.crew == crew,stage.channel == channel {
                let finished=stageFile("finished",stage.id)
                if FileManager.default.fileExists(atPath:finished.path){try Self.privateFile(finished);guard String(decoding:try Data(contentsOf:finished),as:UTF8.self) == stage.id.uuidString else{throw StorageError.refused("文档阶段结束记录不符。")}}
                else{stages.append(stage)}
            }
        }
        guard stages.count <= 1 else{throw StorageError.refused("本群有多份未结束文档保存记录，请保留并核对。")}
        return stages.first
    }
    func prepareDocumentStage(crew:String,channel:String,title:String,body:String,context:CrewdValue)throws->DocumentStage {
        if let original=try currentDocumentStage(crew:crew,channel:channel){return original}
        let stage=DocumentStage(id:UUID(),scope:scope,crew:crew,channel:channel,title:title,body:body,context:context)
        try Self.immutableWrite(encoder.encode(stage),to:stageFile("inputs",stage.id));return stage
    }
    func documentStageAttempted(_ stage:DocumentStage)throws->Bool {
        try checkStage(stage);let file=stageFile("attempts",stage.id)
        guard FileManager.default.fileExists(atPath:file.path) else{return false}
        try Self.privateFile(file)
        guard String(decoding:try Data(contentsOf:file),as:UTF8.self) == stage.id.uuidString else{throw StorageError.refused("文档原保存请求身份不符。")}
        return true
    }
    func markDocumentStageAttempted(_ stage:DocumentStage)throws {
        try checkStage(stage)
        guard try !documentStageAttempted(stage) else{throw StorageError.refused("原保存请求已送交或未知，只能查询原UUID。")}
        try Self.immutableWrite(Data(stage.id.uuidString.utf8),to:stageFile("attempts",stage.id))
    }
    func documentStageResult(_ stage:DocumentStage)throws->CrewdValue? {
        try checkStage(stage);let file=stageFile("results",stage.id)
        guard FileManager.default.fileExists(atPath:file.path) else{return nil}
        try Self.privateFile(file);let result=try decoder.decode(CrewdValue.self,from:Data(contentsOf:file))
        try Self.validateDocumentStageResult(result,stage:stage);return result
    }
    private static func validateDocumentStageResult(_ result:CrewdValue,stage:DocumentStage)throws {
        let c=result["context"],original=stage.context,reference=result["created"]["document"]["object"]["reference"]
        guard result["requestId"].string.flatMap(UUID.init(uuidString:)) == stage.id,["committed","rejected","conflicted"].contains(result["phase"].string ?? ""),
              ["authorityId","dbId","restoreEpoch","crewId","namespace"].allSatisfy({c[$0] == original[$0]}) else{throw StorageError.refused("文档原保存回执身份不完整；保留原阶段和UUID。")}
        if result["phase"] == .string("committed") {
            guard result["beforeGeneration"] == original["generation"],result["afterGeneration"] == c["generation"],result["created"]["document"]["kind"] == .string("object"),reference["kind"] == .string("view"),reference["namespace"] == c["namespace"],reference["id"].string?.isEmpty == false else{throw StorageError.refused("文档原保存回执缺少正式View身份；只可核对原请求。")}
        }
    }
    func recordDocumentStageResult(_ result:CrewdValue,stage:DocumentStage)throws {
        try checkStage(stage);try Self.validateDocumentStageResult(result,stage:stage)
        if let old=try documentStageResult(stage){guard old == result else{throw StorageError.refused("原文档已有不同保存回执。")};return}
        try Self.immutableWrite(encoder.encode(result),to:stageFile("results",stage.id))
    }
    func finishDocumentStage(_ stage:DocumentStage)throws {
        try checkStage(stage);guard try documentStageResult(stage) != nil else{throw StorageError.refused("原文档保存结果尚未确认。")}
        let file=stageFile("finished",stage.id)
        if FileManager.default.fileExists(atPath:file.path){try Self.privateFile(file);return}
        try Self.immutableWrite(Data(stage.id.uuidString.utf8),to:file)
    }
    static let resourceMutationMethods:Set<String>=["todo.assign","todo.setState","humanTask.create","channel.workTagsSet","channel.knowledgeLink","channel.knowledgeUnlink"]
    private static func resourceObject(_ params:CrewdValue)->String? {
        if params["source"] != .null {
            let c=params["source"]["context"],r=params["source"]["reference"]
            let parts=[c["authorityId"],c["dbId"],c["restoreEpoch"],c["crewId"],c["namespace"],r["namespace"],r["kind"],r["id"]].compactMap(\.string)
            guard parts.count == 8,parts.allSatisfy({!$0.isEmpty}) else{return nil}
            return parts.map{"\($0.utf8.count):\($0)"}.joined()
        }
        if params["object"] != .null{return params["object"]["kind"].string.flatMap{kind in params["object"]["id"].string.map{kind+":"+$0}}}
        return nil
    }
    static func validateResourceResult(_ value:CrewdValue,method:String,params:CrewdValue)throws {
        func require(_ valid:Bool)throws{if !valid{throw CrewdClientError(message:"原资源回执结构或身份与原输入不符；保留原请求核对。")}}
        func revision(_ value:CrewdValue)->Bool{guard let s=value.string,!s.isEmpty,s.allSatisfy({$0.isASCII && $0.isNumber}) else{return false};return s == "0" || s.first != "0"}
        try require(resourceMutationMethods.contains(method))
        if method.hasPrefix("channel.knowledge") {
            try require(value["channelId"] == params["channelId"] && value["source"] == params["source"] && value["linked"] == .bool(method == "channel.knowledgeLink") && revision(value["linkRevision"]))
        }else if method == "channel.workTagsSet" {
            try require(value["channelId"] == params["channelId"] && value["object"] == params["object"] && value["tags"] == params["tags"] && revision(value["linkRevision"]))
            guard case .array(let tags)=value["tags"],tags.allSatisfy({$0.string != nil}) else{throw CrewdClientError(message:"标签回执不完整；保留原请求核对。")}
        }else{
            let todo=method == "humanTask.create" ? value["todo"]:value
            let object=method == "humanTask.create" ? todo["object"]:params["object"]
            try require(todo["object"] == object && ["requirement","humanTask"].contains(object["kind"].string ?? "") && object["id"].string?.isEmpty == false && todo["crewId"].string?.isEmpty == false && revision(todo["revision"]) && ["requirement","agentAsk","humanTask"].contains(todo["origin"].string ?? ""))
            let owner=todo["owner"]
            try require(owner == .null || owner["kind"] == .string("human") || (owner["kind"] == .string("session") && owner["id"].string?.isEmpty == false))
            try require(todo["manualState"] == .null || ["open","done"].contains(todo["manualState"].string ?? ""))
            if method == "todo.assign"{try require(todo["owner"] == params["owner"])}
            if method == "todo.setState"{try require(todo["manualState"] == params["state"] && todo["origin"] != .string("agentAsk"))}
            if method == "humanTask.create" {
                let id=value["ask"]["ask"].string ?? value["ask"]["id"].string ?? ""
                try require(value["requestId"] == params["requestId"] && value["channelAssociation"]["channelId"] == params["channelId"] && value["ask"]["question"] == params["text"] && value["ask"]["crewId"] == params["crewId"] && todo["crewId"] == params["crewId"] && todo["origin"] == .string("humanTask") && object["kind"] == .string("humanTask") && object["id"] == .string(id) && id.hasPrefix("H") && !id.dropFirst().isEmpty && id.dropFirst().allSatisfy({$0.isASCII && $0.isNumber}))
                guard case .bool = value["reused"],case .bool = value["channelAssociation"]["linked"] else{throw CrewdClientError(message:"人待办回执不完整；保留原请求核对。")}
            }
        }
    }
    /// Durable original-resource boundary shared by normal submit and recovery.
    func performResourceMutation(_ method:String,params:CrewdValue,crew:String,call:(String,CrewdValue)async throws->CrewdValue)async throws->CrewdValue {
        guard Self.resourceMutationMethods.contains(method),params["requestId"].string.flatMap(UUID.init(uuidString:)) != nil,params["channelId"].string?.isEmpty == false else{throw CrewdClientError(message:"原资源请求身份不完整，尚未提交。",kind:.notSubmitted)}
        let object=Self.resourceObject(params)
        let unresolved=try unknownRecords().contains{old in
            let sameMethod=old.method == method || (old.method.hasPrefix("channel.knowledge") && method.hasPrefix("channel.knowledge"))
            return old.scope == scope && sameMethod && old.params["channelId"] == params["channelId"] && Self.resourceObject(old.params) == object
        }
        guard !unresolved else{throw CrewdClientError(message:"原资源请求结果未确认，请查询原请求；当前输入保留，尚未发送新操作。",kind:.notSubmitted)}
        let prepared=try prepare(method:method,params:params,crew:crew)
        do {
            let result=try await call(method,params)
            try Self.validateResourceResult(result,method:method,params:params)
            try finish(prepared,state:"replyObserved")
            return result
        }catch{
            try? finish(prepared,state:CrewdClientError.isUncertainSubmission(error) ? "unknown":"refused",detail:error.localizedDescription)
            throw error
        }
    }
    /// Redacted notification attempts retain only a token digest, never the
    /// APNs token or provider key. Reading history does not confirm a mutation.
    func personalDraftHistory()throws->[Prepared]{try records(includeObservedFirstMessages:false,includePersonalDrafts:true).filter{$0.method=="command.submit" && $0.params["command"]["operation"].string?.hasPrefix("personalDraft.")==true}.sorted{$0.preparedAt < $1.preparedAt}}
    func notificationHistory()throws->[Prepared]{try records(includeObservedFirstMessages:false,includeNotifications:true).filter{["notification.installationRegister","notification.installationRevoke"].contains($0.method)}.sorted{$0.preparedAt < $1.preparedAt}}
    func unknownRecords() throws -> [Prepared] { try records(includeObservedFirstMessages: false) }
    /// Original successful-observation snapshots remain available for lookup;
    /// their existence is not a durable server success or a replay permission.
    func firstMessageRecords() throws -> [Prepared] {
        Array(try records(includeObservedFirstMessages: true)
            .filter { $0.method == "command.submit" && $0.params["command"]["operation"].string == "crew.createFirstMessage" }
            .sorted { $0.preparedAt > $1.preparedAt }.prefix(64))
    }
    struct CodexChoice:Codable{let scope:Scope;let key:CrewdValue;let revision:UUID;let value:CrewdValue}
    func codexChoice(_ key:CrewdValue)throws->CodexChoice?{
        let file=directory.appendingPathComponent("codex-input-choices").appendingPathComponent(Self.hash(try encoder.encode(scope)+encoder.encode(key))+".json")
        guard FileManager.default.fileExists(atPath:file.path) else{return nil};try Self.privateFile(file)
        let row=try decoder.decode(CodexChoice.self,from:Data(contentsOf:file));guard row.scope==scope,row.key==key else{throw CrewdPublicWire.invalid()};return row
    }
    func saveCodexChoice(_ value:CrewdValue,key:CrewdValue,expected:UUID?)throws->UUID{
        let old=try codexChoice(key);guard old?.revision==expected else{throw CrewdClientError(message:"其它窗口已改变原输入选择；请重新读取，不会覆盖。",kind:.notSubmitted)}
        let id=UUID(),record=CodexChoice(scope:scope,key:key,revision:id,value:value),file=directory.appendingPathComponent("codex-input-choices").appendingPathComponent(Self.hash(try encoder.encode(scope)+encoder.encode(key))+".json")
        let temp=file.deletingLastPathComponent().appendingPathComponent(".pending-"+UUID().uuidString);defer{try? FileManager.default.removeItem(at:temp)}
        try Self.immutableWrite(encoder.encode(record),to:temp)
        guard rename(temp.path,file.path)==0 else{throw CrewdPublicWire.invalid()}
        let fd=open(file.deletingLastPathComponent().path,O_RDONLY|O_DIRECTORY|O_NOFOLLOW);guard fd>=0 else{throw CrewdPublicWire.invalid()};defer{close(fd)};guard fsync(fd)==0 else{throw CrewdPublicWire.invalid()};return id
    }
    func recordCodexInputOriginal(_ data:Data,record:Prepared)throws{
        guard record.scope==scope,record.method=="session.codex.inputPrepare",data.count==record.params["bytes"].int,data.count<=4*1024*1024,Self.hash(data)==record.params["sha256"].string else{throw CrewdPublicWire.invalid()}
        let folder=directory.appendingPathComponent("codex-input-originals")
        var total=0
        for file in try FileManager.default.contentsOfDirectory(at:folder,includingPropertiesForKeys:nil){try Self.privateFile(file);total += (try FileManager.default.attributesOfItem(atPath:file.path)[.size] as? NSNumber)?.intValue ?? 0}
        guard total+data.count<=256*1024*1024 else{throw CrewdClientError(message:"本机输入保全已达到上限；这份文件尚未加入，未上传。请保留原文件并导出已有原输入。",kind:.notSubmitted)}
        try Self.immutableWrite(data,to:folder.appendingPathComponent(record.id.uuidString.lowercased()+".input"))
    }
    func codexInputOriginal(_ record:Prepared)throws->Data{
        guard record.scope==scope,record.method=="session.codex.inputPrepare" else{throw CrewdPublicWire.invalid()}
        let file=directory.appendingPathComponent("codex-input-originals").appendingPathComponent(record.id.uuidString.lowercased()+".input");try Self.privateFile(file)
        guard ((try FileManager.default.attributesOfItem(atPath:file.path)[.size] as? NSNumber)?.intValue ?? Int.max)<=4*1024*1024 else{throw CrewdPublicWire.invalid()}
        let bytes=try Data(contentsOf:file);guard bytes.count==record.params["bytes"].int,Self.hash(bytes)==record.params["sha256"].string else{throw CrewdPublicWire.invalid()};return bytes
    }
    struct CodexArchive:Identifiable{var id:String{Self.scopeID(journal.scope)}
        private static func scopeID(_ scope:Scope)->String{let e=JSONEncoder();e.outputFormatting=[.sortedKeys];return CrewdPresentationJournal.hash((try? e.encode(scope)) ?? Data())};let journal:CrewdPresentationJournal;let records:[Prepared]}
    static func codexArchives(root:URL)throws->[CodexArchive]{
        guard FileManager.default.fileExists(atPath:root.path) else{return []};try privateDirectory(root)
        let folders=try FileManager.default.contentsOfDirectory(at:root,includingPropertiesForKeys:nil);guard folders.count<=512 else{throw CrewdPublicWire.invalid()}
        var result:[CodexArchive]=[]
        for folder in folders{
            guard let namespace=UUID(uuidString:folder.lastPathComponent),FileManager.default.fileExists(atPath:folder.appendingPathComponent("codex-input-originals").path) else{continue};try privateDirectory(folder)
            let prepared=folder.appendingPathComponent("prepared");try privateDirectory(prepared)
            let files=try FileManager.default.contentsOfDirectory(at:prepared,includingPropertiesForKeys:nil);guard files.count<=10000 else{throw CrewdPublicWire.invalid()}
            var rows:[Prepared]=[]
            let decoder=JSONDecoder();decoder.dateDecodingStrategy = .iso8601
            for file in files{try privateFile(file);let row=try decoder.decode(Prepared.self,from:Data(contentsOf:file));guard row.format==1,row.scope.namespace==namespace,file.lastPathComponent==row.id.uuidString.lowercased()+".json" else{throw CrewdPublicWire.invalid()};if row.method=="session.codex.inputPrepare",FileManager.default.fileExists(atPath:folder.appendingPathComponent("codex-input-originals").appendingPathComponent(row.id.uuidString.lowercased()+".input").path){rows.append(row)}}
            let encoder=JSONEncoder();encoder.outputFormatting=[.sortedKeys]
            let grouped=try Dictionary(grouping:rows,by:{Self.hash(try encoder.encode($0.scope))})
            for group in grouped.values{if let first=group.first{let journal=try CrewdPresentationJournal(root:root,scope:first.scope,readOnly:true);result.append(.init(journal:journal,records:group))}}
        };return result
    }
    func codexInputRecords()throws->[Prepared]{try records(includeObservedFirstMessages:false,includeCodexInputs:true).filter{$0.method.hasPrefix("session.codex.input") || $0.method=="command.submit"}}
    func codexInputResult(_ record:Prepared)throws->CrewdValue?{
        guard record.scope==scope else{throw CrewdPublicWire.invalid()}
        let file=directory.appendingPathComponent("codex-input-results").appendingPathComponent(record.id.uuidString.lowercased()+".json")
        guard FileManager.default.fileExists(atPath:file.path) else{return nil};try Self.privateFile(file)
        let wrapper=try decoder.decode(CrewdValue.self,from:Data(contentsOf:file))
        let original=directory.appendingPathComponent("prepared").appendingPathComponent(record.id.uuidString.lowercased()+".json");try Self.privateFile(original)
        let originalBytes=try Data(contentsOf:original),saved=try decoder.decode(Prepared.self,from:originalBytes)
        guard saved.id==record.id,saved.scope==record.scope,saved.method==record.method,saved.params==record.params,wrapper["preparedSHA256"] == .string(Self.hash(originalBytes)) else{throw CrewdPublicWire.invalid()}
        try CrewdCodexInputWire.staging(wrapper["result"],record:record);return wrapper["result"]
    }
    func recordCodexInputResult(_ value:CrewdValue,record:Prepared)throws{
        guard record.scope==scope else{throw CrewdPublicWire.invalid()};try CrewdCodexInputWire.staging(value,record:record)
        if let old=try codexInputResult(record){guard old==value else{throw CrewdPublicWire.invalid()};return}
        let original=directory.appendingPathComponent("prepared").appendingPathComponent(record.id.uuidString.lowercased()+".json");try Self.privateFile(original)
        let wrapper=CrewdValue.object(["preparedSHA256":.string(Self.hash(try Data(contentsOf:original))),"result":value])
        try Self.immutableWrite(encoder.encode(wrapper),to:directory.appendingPathComponent("codex-input-results").appendingPathComponent(record.id.uuidString.lowercased()+".json"))
    }
    func validateUnusedCodexInputs(_ command:CrewdValue)throws{
        let fields=command["params"],selection=fields["codexInputs"] == .null ? fields["inputs"]:fields["codexInputs"]
        if selection == .null{return};_ = try CrewdCodexInputWire.selection(selection)
        let wanted=Set(selection["inputs"].array.compactMap{$0["inputId"].string});if wanted.isEmpty{return}
        for record in try codexInputRecords() where record.scope==scope{
            if record.method=="command.submit"{
                let p=record.params["command"]["params"],old=p["codexInputs"] == .null ? p["inputs"]:p["codexInputs"]
                if !wanted.isDisjoint(with:old["inputs"].array.compactMap{$0["inputId"].string}){throw CrewdClientError(message:"原输入已被任务尝试引用；请核对原任务，不能重新使用。",kind:.notSubmitted)}
            }else if record.method=="session.codex.inputDiscard",let id=record.params["input"]["inputId"].string,wanted.contains(id){throw CrewdClientError(message:"原输入已请求丢弃；请只读核对原请求。",kind:.notSubmitted)}
        }
    }
    private func records(includeObservedFirstMessages: Bool, includeNotifications:Bool=false,includeCodexInputs:Bool=false,includePersonalDrafts:Bool=false) throws -> [Prepared] {
        var rows: [Prepared] = []
        for file in try FileManager.default.contentsOfDirectory(at: directory.appendingPathComponent("prepared"), includingPropertiesForKeys: nil).sorted(by: { $0.path < $1.path }) {
            guard file.pathExtension == "json" else { throw StorageError.refused("前端历史目录包含未知对象；保留原资料。") }
            try Self.privateFile(file)
            let bytes = try Data(contentsOf: file), record = try decoder.decode(Prepared.self, from: bytes)
            guard record.format == 1, record.scope.namespace == scope.namespace, file.lastPathComponent == record.id.uuidString.lowercased() + ".json" else { throw StorageError.refused("原前端历史身份或来源不符。") }
            let result = directory.appendingPathComponent("outcomes").appendingPathComponent(file.lastPathComponent)
            var observed: Outcome?
            if FileManager.default.fileExists(atPath: result.path) {
                try Self.privateFile(result)
                let outcome = try decoder.decode(Outcome.self, from: Data(contentsOf: result))
                guard outcome.format == 1, outcome.observationID == record.id, outcome.preparedSHA256 == Self.hash(bytes),
                      ["unknown", "replyObserved", "refused"].contains(outcome.state) else { throw StorageError.refused("原前端历史结果与提交快照不符。") }
                observed = outcome
            }
            let reconciled = directory.appendingPathComponent("reconciled").appendingPathComponent(file.lastPathComponent)
            if FileManager.default.fileExists(atPath: reconciled.path) {
                try Self.privateFile(reconciled)
                let observation = try decoder.decode(Outcome.self, from: Data(contentsOf: reconciled))
                guard observation.format == 1, observation.observationID == record.id,
                      observation.preparedSHA256 == Self.hash(bytes), observation.state == "replyObserved" else { throw StorageError.refused("终态核对与原记录不符。") }
                observed = observation
            }
            guard let outcome = observed else { rows.append(record); continue }
            if includePersonalDrafts,record.method=="command.submit",record.params["command"]["operation"].string?.hasPrefix("personalDraft.")==true{rows.append(record)}
            else if includeCodexInputs, record.method.hasPrefix("session.codex.input") || record.method == "command.submit" {rows.append(record)}
            else if includeNotifications, ["notification.installationRegister","notification.installationRevoke"].contains(record.method) {rows.append(record)}
            else if outcome.state == "unknown" { rows.append(record) }
            else if includeObservedFirstMessages, outcome.state == "replyObserved",
                    record.method == "command.submit", record.params["command"]["operation"].string == "crew.createFirstMessage" { rows.append(record) }
            else if !["replyObserved", "refused"].contains(outcome.state) { throw StorageError.refused("原前端历史出现未知状态。") }
        }
        return rows
    }
}
