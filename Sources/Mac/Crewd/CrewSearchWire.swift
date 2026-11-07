import Foundation

/// Reject malformed collections and ambiguous identities before exposing results.
enum CrewSearchWire {
    static func rows(_ value:CrewdValue,id:(CrewdValue)->String?) throws->[CrewdValue]{
        guard case .array(let rows)=value else{throw CrewdPublicWire.invalid()}
        var seen=Set<String>()
        for row in rows{guard let key=id(row),!key.isEmpty,seen.insert(key).inserted else{throw CrewdPublicWire.invalid()}}
        return rows
    }
}

