import Foundation

/// Window-owned submission state only. No persisted requirement/status copy.
struct CrewdRequirementDrafts {
    struct Snapshot: Equatable {
        let crew:String;let text:String;let requestID:UUID;let revision:UUID;let channel:String
        init(crew:String,text:String,requestID:UUID,revision:UUID,channel:String=""){self.crew=crew;self.text=text;self.requestID=requestID;self.revision=revision;self.channel=channel}
        var key:Key{Key(crew:crew,channel:channel)}
    }
    struct Key:Hashable {let crew:String;let channel:String}
    private var entries:[Key:Snapshot]=[:]
    private var submitting:[Key:UUID]=[:]
    private var unknown:[Key:[Snapshot]]=[:]
    func uncertain(_ crew:String,channel:String="")->[Snapshot] {unknown[Key(crew:crew,channel:channel)] ?? []}
    mutating func confirm(_ snapshot:Snapshot)->Bool {
        guard !busy(snapshot.crew,channel:snapshot.channel), uncertain(snapshot.crew,channel:snapshot.channel).contains(snapshot) else {return false}
        submitting[snapshot.key]=snapshot.requestID;return true
    }
    func text(_ crew:String,channel:String="")->String {entries[Key(crew:crew,channel:channel)]?.text ?? ""}
    func busy(_ crew:String,channel:String="")->Bool {submitting[Key(crew:crew,channel:channel)] != nil}
    mutating func set(_ crew:String,channel:String="",text:String) {
        guard self.text(crew,channel:channel) != text else {return}
        entries[Key(crew:crew,channel:channel)]=Snapshot(crew:crew,text:text,requestID:UUID(),revision:UUID(),channel:channel)
    }
    mutating func begin(_ crew:String,channel:String="")->Snapshot? {
        guard !crew.isEmpty,!busy(crew,channel:channel),let snapshot=entries[Key(crew:crew,channel:channel)],!snapshot.text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty else {return nil}
        submitting[snapshot.key]=snapshot.requestID;return snapshot
    }
    /// Failure includes transport-uncertain outcome: keep the identical request identity.
    mutating func finish(_ snapshot:Snapshot,accepted:Bool,uncertain:Bool=false) {
        if accepted {unknown[snapshot.key]?.removeAll {$0.requestID == snapshot.requestID}}
        else if uncertain && !(unknown[snapshot.key] ?? []).contains(snapshot) {unknown[snapshot.key,default:[]].append(snapshot)}
        if submitting[snapshot.key] == snapshot.requestID {submitting[snapshot.key]=nil}
        if accepted && entries[snapshot.key] == snapshot {entries[snapshot.key]=nil}
    }
}

/// Creation input only, scoped by its owning workspace. Crew creation has no
/// server idempotency key: uncertain submissions require a fresh roster review.
struct CrewdCrewCreationDrafts {
    struct Fields: Equatable { var title = "", directory = "", parent = "" }
    struct Snapshot: Equatable { let fields: Fields; let version: UUID }
    private var current: Snapshot?
    private var pending: UUID?
    private(set) var unknown: [Snapshot] = []
    var fields: Fields { current?.fields ?? Fields() }
    var busy: Bool { pending != nil }
    mutating func edit(_ value: Fields) { if fields != value { current = Snapshot(fields: value, version: UUID()) } }
    mutating func begin() -> Snapshot? {
        guard !busy, let current, !current.fields.title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              !current.fields.directory.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        pending = current.version; return current
    }
    mutating func finish(_ snapshot: Snapshot, accepted: Bool, uncertain: Bool = false) {
        if pending == snapshot.version { pending = nil }
        if uncertain, !unknown.contains(where: { $0.version == snapshot.version }) { unknown.append(snapshot) }
        if accepted {
            unknown.removeAll { $0.version == snapshot.version }
            if current?.version == snapshot.version { current = nil }
        }
    }
}
