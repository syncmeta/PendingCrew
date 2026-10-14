import Foundation

/// Read-only navigation projections. Parent links are hierarchy; folders are a different
/// backend feature. A DAG crew can appear below multiple parents with distinct row paths.
enum CrewdNavigation {
    enum Sidebar: String, CaseIterable { case contacts = "通讯录", messages = "消息" }
    enum Tab: String, CaseIterable { case messages = "消息", todo = "Todo", documents = "文档", progress = "进展" }
    struct Row: Identifiable, Equatable { let id: String; let object: String; let depth: Int; let expandable: Bool }
    static func directory(_ crews: [CrewdValue], collapsed: Set<String>) -> [Row] {
        let map = Dictionary(crews.compactMap { v in v["id"].string.map { ($0,v) } }, uniquingKeysWith: {a,_ in a})
        let roots = crews.compactMap { c -> String? in
            guard let id = c["id"].string else { return nil }
            return c["parentIds"].array.compactMap(\.string).allSatisfy { map[$0] == nil } ? id : nil
        }
        var out: [Row] = []
        func walk(_ id: String, _ path: [String]) {
            guard let c = map[id], !path.contains(id) else { return }
            let next = path + [id], key = next.map { "\($0.utf8.count):\($0)" }.joined()
            let children = c["childIds"].array.compactMap(\.string).filter { map[$0] != nil }
            out.append(Row(id:key,object:id,depth:path.count,expandable:!children.isEmpty))
            if !collapsed.contains(key) { for child in children { walk(child,next) } }
        }
        for id in roots { walk(id,[]) }
        return out
    }
    static func timeline(_ crews: [CrewdValue], activity: [String: CrewdValue]) -> [CrewdValue] {
        crews.sorted { a,b in
            let x=activity[a["id"].string ?? ""]?["createdAt"].string, y=activity[b["id"].string ?? ""]?["createdAt"].string
            if x != y { return (x ?? "") > (y ?? "") }
            return (a["position"].int ?? 0) < (b["position"].int ?? 0)
        }
    }
    static func planTree(_ plans: [CrewdPlan], collapsed: Set<String>) -> [Row] {
        let map = Dictionary(plans.map {($0.id.raw,$0)},uniquingKeysWith:{a,_ in a})
        var out:[Row]=[]
        func walk(_ id:String,_ path:[String]) {
            guard !path.contains(id) else { return }
            let children=plans.filter {$0.value["parent"].string == id}.map { $0.id.raw }
            out.append(Row(id:id,object:id,depth:path.count,expandable:!children.isEmpty))
            if !collapsed.contains(id) { for child in children {walk(child,path+[id])} }
        }
        for plan in plans where plan.value["parent"].string.map({map[$0] == nil}) ?? true {walk(plan.id.raw,[])}
        return out
    }
}
/// Stable real participant identity, sharing the original PendingCrew/PendingBot pool.
/// No membership inference and no padding to three fake actors.
enum CrewdAvatarIdentity {
    static func seed(_ member:CrewdValue)->String? {
        if member["kind"].string == "human" {return "human"}
        if member["kind"].string == "session",let id=member["id"].string,!id.isEmpty {return id}
        return nil
    }
    static func group(_ members:[CrewdValue])->[String] {
        Array(Set(members.compactMap(seed)).sorted().prefix(3))
    }
}
