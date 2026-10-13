import Foundation

/// Window-owned submission state only. No persisted requirement/status copy.
struct CrewdRequirementDrafts {
    struct Snapshot: Equatable {let crew:String; let text:String; let requestID:UUID; let revision:UUID}
    private var entries:[String:Snapshot]=[:]
    private var submitting:[String:UUID]=[:]
    private var unknown:[String:[Snapshot]]=[:]
    func uncertain(_ crew:String)->[Snapshot] {unknown[crew] ?? []}
    mutating func confirm(_ snapshot:Snapshot)->Bool {
        guard !busy(snapshot.crew), uncertain(snapshot.crew).contains(snapshot) else {return false}
        submitting[snapshot.crew]=snapshot.requestID;return true
    }
    func text(_ crew:String)->String {entries[crew]?.text ?? ""}
    func busy(_ crew:String)->Bool {submitting[crew] != nil}
    mutating func set(_ crew:String,text:String) {
        guard self.text(crew) != text else {return}
        entries[crew]=Snapshot(crew:crew,text:text,requestID:UUID(),revision:UUID())
    }
    mutating func begin(_ crew:String)->Snapshot? {
        guard !crew.isEmpty,!busy(crew),let snapshot=entries[crew],!snapshot.text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty else {return nil}
        submitting[crew]=snapshot.requestID;return snapshot
    }
    /// Failure includes transport-uncertain outcome: keep the identical request identity.
    mutating func finish(_ snapshot:Snapshot,accepted:Bool,uncertain:Bool=false) {
        if accepted {unknown[snapshot.crew]?.removeAll {$0.requestID == snapshot.requestID}}
        else if uncertain && !(unknown[snapshot.crew] ?? []).contains(snapshot) {unknown[snapshot.crew,default:[]].append(snapshot)}
        if submitting[snapshot.crew] == snapshot.requestID {submitting[snapshot.crew]=nil}
        if accepted && entries[snapshot.crew] == snapshot {entries[snapshot.crew]=nil}
    }
}
