import Foundation
func v(_ text:String)->CrewdValue {try! JSONDecoder().decode(CrewdValue.self,from:Data(text.utf8))}
@main struct NavigationTest {
    static func main() throws {
        let crews=v("[{\"id\":\"a\",\"title\":\"A\",\"parentIds\":[],\"childIds\":[\"c\"],\"position\":0,\"updatedAt\":\"2099-01-01\"},{\"id\":\"b\",\"title\":\"B\",\"parentIds\":[],\"childIds\":[\"c\"],\"position\":1},{\"id\":\"c\",\"title\":\"C\",\"parentIds\":[\"a\",\"b\"],\"childIds\":[]}]").array
        let rows=CrewdNavigation.directory(crews,collapsed:[])
        precondition(rows.map(\.object)==["a","c","b","c"] && Set(rows.map(\.id)).count == 4,"multi-parent retains both original paths")
        let folded=CrewdNavigation.directory(crews,collapsed:[rows[0].id])
        precondition(folded.map(\.object)==["a","b","c"],"fold one parent must not hide the other appearance")
        let messages=["b":v("{\"createdAt\":\"2026-10-13T12:00:00Z\",\"text\":\"real latest\"}")]
        precondition(CrewdNavigation.timeline(crews,activity:messages).first!["id"] == .string("b"),"timeflow must not use crew.updatedAt")
        func p(_ id:String,_ parent:String?)->CrewdPlan {let x=CrewdValue.object(["id":.string(id),"parent":parent.map(CrewdValue.string) ?? .null,"revision":.string("1"),"crewId":.string("a"),"title":.string(id),"status":.string("open"),"sources":.array([])]); return CrewdPlan(id:CrewdWorkID(id,kind:.plan)!,summary:x,detail:nil)}
        let plans=[p("W1",nil),p("W2","W1"),p("W3","W2"),p("W4","W99")]
        let tree=CrewdNavigation.planTree(plans,collapsed:[])
        precondition(tree.map(\.depth)==[0,1,2,0] && tree.map(\.object)==["W1","W2","W3","W4"],"three-level parent tree retains foreign-parent reference as scoped root")
        precondition(CrewdNavigation.planTree(plans,collapsed:["W1"]).map(\.object)==["W1","W4"])
        precondition(CrewdNavigation.Tab.allCases.map(\.rawValue)==["消息","Todo","文档","进展"])
        precondition(CrewdNavigation.Sidebar.allCases.map(\.rawValue)==["通讯录","消息"])
        let snapshot:[String:Any]=["sidebarModes":CrewdNavigation.Sidebar.allCases.map(\.rawValue),"crewTabs":CrewdNavigation.Tab.allCases.map(\.rawValue),"directoryRows":rows.map { ["path":$0.id,"object":$0.object,"depth":$0.depth] as [String:Any]},"planRows":tree.map { ["id":$0.object,"depth":$0.depth] as [String:Any]}]
        let output=try JSONSerialization.data(withJSONObject:snapshot,options:[.prettyPrinted,.sortedKeys])
        if CommandLine.arguments.count > 1 {try output.write(to:URL(fileURLWithPath:CommandLine.arguments[1]))}
        print("PASS: original navigation structure, multi-parent folding, real message ordering and shared plan hierarchy")
    }
}
