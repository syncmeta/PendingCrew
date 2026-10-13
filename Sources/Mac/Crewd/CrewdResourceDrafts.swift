#if os(macOS)
import Foundation

/// Mutable window input, including immutable uncertain submission snapshots.
struct CrewdResourceDrafts {
    enum Domain:String,Hashable {case document,folder}
    struct Key:Hashable {let crew:String;let view:CrewdResourceView;let object:String;let operation:String;var domain:Domain = .document}
    struct Fields:Equatable {var title="",body="",parent="",position=0,folder="",revision=""}
    struct Snapshot:Equatable {let key:Key;let fields:Fields;let request:UUID;let version:UUID}
    private var entries:[Key:Snapshot]=[:]
    private var selections:[String:[CrewdResourceView:String]]=[:]
    func selection(_ crew:String,_ view:CrewdResourceView)->String {selections[crew]?[view] ?? "all"}
    mutating func select(_ crew:String,_ view:CrewdResourceView,_ value:String){selections[crew,default:[:]][view]=value}
    private var pending:[Key:UUID]=[:]
    private var unknown:[Key:[Snapshot]]=[:]
    func exists(_ key:Key)->Bool {entries[key] != nil}
    func fields(_ key:Key)->Fields {entries[key]?.fields ?? Fields()}
    func busy(_ key:Key)->Bool {pending[key] != nil}
    func uncertain(_ key:Key)->[Snapshot] {unknown[key] ?? []}
    mutating func edit(_ key:Key,_ fields:Fields){guard entries[key]?.fields != fields else{return};entries[key]=Snapshot(key:key,fields:fields,request:UUID(),version:UUID())}
    mutating func begin(_ key:Key)->Snapshot? {guard !busy(key),let snapshot=entries[key] else{return nil};pending[key]=snapshot.version;return snapshot}
    mutating func confirm(_ snapshot:Snapshot)->Bool {guard !busy(snapshot.key),uncertain(snapshot.key).contains(snapshot) else{return false};pending[snapshot.key]=snapshot.version;return true}
    mutating func finish(_ snapshot:Snapshot,accepted:Bool,uncertain:Bool=false){
        if pending[snapshot.key] == snapshot.version {pending[snapshot.key]=nil}
        if accepted {unknown[snapshot.key]?.removeAll{$0.version == snapshot.version}}
        else if uncertain && !(unknown[snapshot.key] ?? []).contains(snapshot){unknown[snapshot.key,default:[]].append(snapshot)}
        if accepted && entries[snapshot.key] == snapshot {entries[snapshot.key]=nil}
    }
}
#endif
