import Foundation
struct Samples: Decodable {
    let workspace: CrewdDataWorkspaceInfo
    let submit: CrewdDataChangeResult
    let objectDetail: CrewdDataObjectDetail
    let preview: CrewdDataPreview
    let snapshots: CrewdDataSnapshots
}
@main struct WireRegression {
    static func main() throws {
        let sample = try JSONDecoder().decode(Samples.self,from:Data(contentsOf:URL(fileURLWithPath:CommandLine.arguments[1])))
        guard case .view(_,_,_,.web(_,_,_,_,_,let bindings,_,_),_,_) = sample.objectDetail.object else { fatalError("missing real web view") }
        precondition(!bindings.isEmpty && sample.workspace.context.namespace == sample.objectDetail.context.namespace)
        precondition(sample.submit.phase == .committed && sample.submit.afterGeneration == sample.objectDetail.generation)
        _ = try JSONEncoder().encode(sample.objectDetail)
        print("PASS Swift decodes actual backend serde workspace/submit/web object+bindings/preview/snapshots without Dolt context")
    }
}
