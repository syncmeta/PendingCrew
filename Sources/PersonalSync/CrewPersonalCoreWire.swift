import Foundation

/// Core personal values are inert, scoped drafts. No media or credential fields.
enum CrewPersonalCoreWire {
    typealias Payload=CrewComposerDraftStore.Draft
    static func invalid()->CrewdClientError{.init(message:"个人 Core 草稿契约或原回执不符；原文保留，请核对原请求。")}
    static func fields(_ v:CrewdValue,_ names:Set<String>)throws{guard case .object(let f)=v,Set(f.keys)==names else{throw invalid()}}
    static func text(_ v:CrewdValue)throws->String{try CrewdPublicWire.token(v)}
    static func uuid(_ v:CrewdValue)throws->String{let s=try text(v);guard UUID(uuidString:s)?.uuidString.lowercased()==s else{throw invalid()};return s}
    static func revision(_ v:CrewdValue,zero:Bool=false)throws->UInt64{let s=try text(v);guard let n=UInt64(s),String(n)==s,zero || n>0 else{throw invalid()};return n}
    static func nullable(_ v:CrewdValue)throws->String?{v == .null ? nil:try text(v)}
    static func bool(_ v:CrewdValue)throws->Bool{try CrewdPublicWire.boolean(v)}
    struct Member:Equatable,Sendable {
        let id:String,writer:String,revision:String
        let grant:String?,grantRevision:String?,spki:String?
        let optedIn:Bool,revoked:Bool
        init(_ v:CrewdValue)throws{
            try fields(v,["memberId","writerId","revision","grantId","grantRevision","clientSpkiSha256","optedIn","revoked"])
            id=try uuid(v["memberId"]);writer=try uuid(v["writerId"]);_ = try CrewPersonalCoreWire.revision(v["revision"]);revision=try text(v["revision"])
            grant=try nullable(v["grantId"]);grantRevision=try nullable(v["grantRevision"]);spki=try nullable(v["clientSpkiSha256"])
            guard (grant==nil && grantRevision==nil && spki==nil) || (grant != nil && grantRevision != nil && spki?.count==64 && spki!.allSatisfy({"0123456789abcdef".contains($0)})) else{throw invalid()}
            optedIn=try bool(v["optedIn"]);revoked=try bool(v["revoked"])
        }
    }
    struct Status:Equatable,Sendable {
        let database:String,owner:Bool,ownerEpoch:String?,available:Bool,reason:String?,space:String?,epoch:String?,revision:String,member:Member?
        init(_ v:CrewdValue)throws{
            try fields(v,["dbId","owner","ownerEpoch","available","reason","spaceId","spaceEpoch","spaceRevision","member","payloadLimitBytes","spaceLimitBytes","branchLimit","memberLimit"])
            database=try text(v["dbId"]);owner=try bool(v["owner"]);ownerEpoch=try nullable(v["ownerEpoch"]);available=try bool(v["available"]);reason=try nullable(v["reason"])
            guard reason.map({["notConfigured","notApproved","optedOut","restoreRequired"].contains($0)}) ?? true,!owner || ownerEpoch != nil,owner || ownerEpoch==nil else{throw invalid()}
            space=v["spaceId"] == .null ? nil:try uuid(v["spaceId"]);epoch=try nullable(v["spaceEpoch"]);_ = try CrewPersonalCoreWire.revision(v["spaceRevision"],zero:true);revision=try text(v["spaceRevision"])
            member=v["member"] == .null ? nil:try Member(v["member"])
            guard v["payloadLimitBytes"].int==65536,v["spaceLimitBytes"].int==8388608,v["branchLimit"].int==4096,v["memberLimit"].int==64,
                !available || (space != nil && epoch != nil && member?.optedIn==true && member?.revoked==false && reason==nil) else{throw invalid()}
        }
        var projection:PersonalCoreDraftProjection?{guard let space,let epoch,let member,!member.revoked else{return nil};return .init(databaseID:database,spaceID:space,spaceEpoch:epoch,memberID:member.id,writerID:member.writer)}
        var scope:CrewdValue?{guard let space,let epoch,let member else{return nil};return .object(["spaceId":.string(space),"spaceEpoch":.string(epoch),"memberId":.string(member.id),"memberRevision":.string(member.revision)])}
    }
    static func key(_ key:AppleDraftKey)throws->CrewdValue{
        guard key.authorityID==key.databaseID,["channelMessage","sessionMessage","initialTask"].contains(key.kind) else{throw invalid()}
        let destination:CrewdValue
        switch key.destination{case .channel(let id):guard key.kind != "sessionMessage" else{throw invalid()};destination = .object(["kind":.string("channel"),"id":.string(id)])
        case .originalSession(let id):guard key.kind == "sessionMessage" else{throw invalid()};destination = .object(["kind":.string("originalSession"),"id":.string(id)])}
        return .object(["authorityId":.string(key.authorityID),"dbId":.string(key.databaseID),"crewId":.string(key.crewID),"destination":destination,"kind":.string(key.kind)])
    }
    static func key(_ v:CrewdValue)throws->AppleDraftKey{
        try fields(v,["authorityId","dbId","crewId","destination","kind"]);try fields(v["destination"],["kind","id"])
        let id=try text(v["destination"]["id"]),destination:AppleDraftKey.Destination
        switch v["destination"]["kind"].string{case "channel":destination = .channel(id);case "originalSession":destination = .originalSession(id);default:throw invalid()}
        let result=AppleDraftKey(authorityID:try text(v["authorityId"]),databaseID:try text(v["dbId"]),crewID:try text(v["crewId"]),destination:destination,kind:try text(v["kind"]))
        guard try key(result)==v else{throw invalid()};return result
    }
    static func mention(_ m:CrewMention)->CrewdValue{.object(["kind":.string(m.kind),"target_id":m.targetId.map(CrewdValue.string) ?? .null])}
    static func payload(_ p:Payload)throws->CrewdValue{
        let reply:CrewdValue=p.replyTarget.map{.object(["mention":$0.mention.map(mention) ?? .null,"replyToId":.string($0.replyToId),"quotedSender":.string($0.quotedSender),"quotedSnippet":.string($0.quotedSnippet)])} ?? .null
        let v:CrewdValue = .object(["text":.string(p.text),"stagedMentions":.array(p.stagedMentions.map{.object(["token":.string($0.token),"mention":mention($0.mention)])}),"replyTarget":reply])
        guard p.stagedMentions.count<=64,try JSONEncoder().encode(v).count<=65536 else{throw invalid()};return v
    }
    static func payload(_ v:CrewdValue)throws->Payload{
        try fields(v,["text","stagedMentions","replyTarget"])
        guard let original=v["text"].string,case .array(let mentions)=v["stagedMentions"],mentions.count<=64 else{throw invalid()}
        func m(_ v:CrewdValue)throws->CrewMention{try fields(v,["kind","target_id"]);return .init(kind:try text(v["kind"]),targetId:try nullable(v["target_id"]))}
        let staged=try mentions.map{v in try fields(v,["token","mention"]);return CrewStagedMention(token:try text(v["token"]),mention:try m(v["mention"]))}
        var reply:CrewReplyTarget?
        if v["replyTarget"] != .null{let r=v["replyTarget"];try fields(r,["mention","replyToId","quotedSender","quotedSnippet"]);guard let sender=r["quotedSender"].string,let snippet=r["quotedSnippet"].string else{throw invalid()};reply = .init(mention:r["mention"] == .null ? nil:try m(r["mention"]),replyToId:try text(r["replyToId"]),quotedSender:sender,quotedSnippet:snippet)}
        let result=Payload(text:original,stagedMentions:staged,replyTarget:reply);guard try payload(result)==v else{throw invalid()};return result
    }
    static func branch(_ v:CrewdValue)throws->AppleBranch{
        try fields(v,["branchId","key","writerId","revision","retired","payload"])
        let retired=try bool(v["retired"]);guard retired == (v["payload"] == .null) else{throw invalid()}
        return .init(id:UUID(uuidString:try uuid(v["branchId"]))!,key:try key(v["key"]),deviceID:try uuid(v["writerId"]),revision:try revision(v["revision"]),payload:retired ? nil:try appleSyncEncode(payload(v["payload"])),retired:retired)
    }
    static func scope(_ v:CrewdValue)throws{try fields(v,["spaceId","spaceEpoch","memberId","memberRevision"]);_ = try uuid(v["spaceId"]);_ = try text(v["spaceEpoch"]);_ = try uuid(v["memberId"]);_ = try revision(v["memberRevision"])}
    static func canonical(_ operation:String,_ v:CrewdValue)throws->CrewdValue{
        switch operation{
        case "personalDraft.ownerConfigure":try fields(v,["ownerEpoch","expectedSpaceId","expectedSpaceRevision","localEnabled","acknowledgeRestore"]);_ = try text(v["ownerEpoch"]);if v["expectedSpaceId"] != .null{_ = try uuid(v["expectedSpaceId"])};_ = try revision(v["expectedSpaceRevision"],zero:true);_ = try bool(v["localEnabled"]);_ = try bool(v["acknowledgeRestore"])
        case "personalDraft.memberSet":try fields(v,["spaceId","spaceEpoch","expectedSpaceRevision","grantId","grantRevision","clientSpkiSha256","enabled"]);_ = try uuid(v["spaceId"]);_ = try text(v["spaceEpoch"]);_ = try revision(v["expectedSpaceRevision"],zero:true);_ = try text(v["grantId"]);_ = try text(v["grantRevision"]);let hash=try text(v["clientSpkiSha256"]);guard hash.count==64,hash.allSatisfy({"0123456789abcdef".contains($0)}) else{throw invalid()};_ = try bool(v["enabled"])
        case "personalDraft.consentSet":try fields(v,["scope","enabled"]);try scope(v["scope"]);_ = try bool(v["enabled"])
        case "personalDraft.checkpoint":try fields(v,["scope","branchId","key","expectedRevision","revision","payload"]);try scope(v["scope"]);_ = try uuid(v["branchId"]);_ = try key(v["key"]);let old=try revision(v["expectedRevision"],zero:true);guard try revision(v["revision"])>old else{throw invalid()};_ = try payload(v["payload"])
        case "personalDraft.resolve":try fields(v,["scope","key","retiring","replacement"]);try scope(v["scope"]);_ = try key(v["key"]);guard case .array(let rows)=v["retiring"],!rows.isEmpty,rows.count<=64 else{throw invalid()};var ids=Set<String>();for row in rows{try fields(row,["branchId","revision"]);guard ids.insert(try uuid(row["branchId"])).inserted else{throw invalid()};_ = try revision(row["revision"])};if v["replacement"] != .null{let r=v["replacement"];try fields(r,["branchId","revision","payload"]);guard !ids.contains(try uuid(r["branchId"])) else{throw invalid()};_ = try revision(r["revision"]);_ = try payload(r["payload"])}
        default:throw invalid()
        };return v
    }
    private static func retirements(_ v:CrewdValue)throws->[String:UInt64]{
        guard case .array(let rows)=v,rows.count<=64 else{throw invalid()};var result:[String:UInt64]=[:]
        for row in rows{try fields(row,["branchId","revision"]);let id=try uuid(row["branchId"]);guard result[id]==nil else{throw invalid()};result[id]=try revision(row["revision"])};return result
    }
    /// Runs before the original journal can finish replyObserved.
    static func validateResult(_ v:CrewdValue,command:CrewdValue,expectedWriter:String?=nil)throws{
        let op=try text(command["operation"]),p=try canonical(op,command["params"])
        try fields(v,["operation","spaceId","spaceEpoch","spaceRevision","member","branch","retired"])
        guard v["operation"] == .string(op) else{throw invalid()};_ = try uuid(v["spaceId"]);_ = try text(v["spaceEpoch"]);_ = try revision(v["spaceRevision"])
        let member=v["member"] == .null ? nil:try Member(v["member"])
        if p["scope"] != .null{guard v["spaceId"]==p["scope"]["spaceId"],v["spaceEpoch"]==p["scope"]["spaceEpoch"] else{throw invalid()}}
        if op=="personalDraft.checkpoint"{let b=try branch(v["branch"]);guard b.id.uuidString.lowercased()==p["branchId"].string,try key(b.key)==p["key"],String(b.revision)==p["revision"].string,!b.retired,let bytes=b.payload,try payload(JSONDecoder().decode(Payload.self,from:bytes))==p["payload"] else{throw invalid()}}
        else if op=="personalDraft.resolve"{guard try retirements(v["retired"])==retirements(p["retiring"]) else{throw invalid()};if p["replacement"] == .null{guard v["branch"] == .null else{throw invalid()}}else{let b=try branch(v["branch"]),r=p["replacement"];guard b.id.uuidString.lowercased()==r["branchId"].string,try key(b.key)==p["key"],String(b.revision)==r["revision"].string,!b.retired,let bytes=b.payload,try payload(JSONDecoder().decode(Payload.self,from:bytes))==r["payload"] else{throw invalid()}}}
        else{guard v["branch"] == .null,v["retired"] == .array([]) else{throw invalid()}}
        if let expectedWriter,v["branch"] != .null{guard try branch(v["branch"]).deviceID==expectedWriter else{throw invalid()}}
        if op=="personalDraft.checkpoint"{guard v["retired"] == .array([]),member==nil else{throw invalid()}}
        if op=="personalDraft.resolve"{guard member==nil else{throw invalid()}}
        if op=="personalDraft.memberSet"{
            let enabled=try bool(p["enabled"])
            guard member?.grant==p["grantId"].string,member?.revoked == !enabled else{throw invalid()}
            if enabled{guard member?.grantRevision==p["grantRevision"].string,member?.spki==p["clientSpkiSha256"].string,member?.optedIn==false else{throw invalid()}}
            else{guard member?.optedIn==false else{throw invalid()}}
        }
        if op=="personalDraft.consentSet"{guard let member,member.id==p["scope"]["memberId"].string,member.optedIn==(try bool(p["enabled"])),!member.revoked,UInt64(member.revision)! > (try revision(p["scope"]["memberRevision"])) else{throw invalid()};if let expectedWriter{guard member.writer==expectedWriter else{throw invalid()}}}
    }
}
