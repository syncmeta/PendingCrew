// Compile the real AppModel.swift; only the external connection/storage seams
// are bounded stubs. This verifies UI routing, not TLS/Keychain qualification.
import Foundation
import SwiftUI
import Combine
@MainActor protocol PendingCrewBackend:AnyObject {}
@MainActor final class CrewdPendingCrewBackend:PendingCrewBackend {
    let workspace:CrewdWorkspace
    init(workspace:CrewdWorkspace){self.workspace=workspace}
}
@MainActor final class SelectionClient {var readyForWork=true;let connectionEpoch=UUID()}
@MainActor final class CrewdWorkspace {
    let authorityRecordID:UUID
    let client=SelectionClient()
    let observationGeneration=UUID()
    var originalJournalRequired=false
    var originalJournalReady=false
    var error:String?
    init(_ id:UUID){authorityRecordID=id}
    func acceptsObservation(_ id:UUID)->Bool{id==observationGeneration}
    func prepareOriginalJournal(root:URL)async{originalJournalReady=true}
}
struct SelectionComputer {let id:UUID;var paired=true}
@MainActor final class CrewdRemoteWindow:ObservableObject {
    @Published var workspace:CrewdWorkspace?
    @Published var error:String?
    var computers:[SelectionComputer]=[]
    var selected:UUID?
    var canForceManagedStop=false
    var refreshWorks=true
    var rejectSelection=false
    var localSelections=0
    var remoteSelections=0
    let localID=UUID()
    func refresh()async->Bool{refreshWorks}
    func select(_ id:UUID)async {
        remoteSelections+=1
        guard !rejectSelection else{error="synthetic invalid receipt";return}
        selected=id;workspace=CrewdWorkspace(id)
    }
    func selectLocal(directory:URL)async{localSelections+=1;selected=nil;workspace=CrewdWorkspace(localID)}
    func exit(mode:Int)async throws{}
}
enum CrewdFrontendConfiguration {static let directory:URL?=URL(fileURLWithPath:"/private/tmp/pcw-selection-fixture-no-io")}
enum CrewdFrontendIsolation {static func prepareAuthority(directory:URL)throws{}}
@MainActor final class AppQuitLifecycle {
    static let shared=AppQuitLifecycle()
    var crewdExit:((Int)async throws->Void)?
    var crewdMayForceStop:(()->Bool)?
}
@main struct EnrollmentSelectionFixture {
    @MainActor static func main()async {
        let model=AppModel();let remote=UUID();model.crewdWindow.computers=[.init(id:remote)]
        #if ENROLLMENT_FIXED_SELECTION
        await model.connectCrewdAuthority(computerID:remote)
        #else
        // The original B URL completion callback, using the real old method.
        await model.crewdWindow.select(remote);await model.connectCrewdAuthority()
        #endif
        precondition(model.crewdWindow.selected==remote && model.crewdBackend?.workspace.authorityRecordID==remote,"explicit enrolled computer was replaced by default local selection")
        precondition(model.crewdWindow.localSelections==0)
        print("PASS exact enrolled record selected and journal/backend bound, no local overwrite")
        #if ENROLLMENT_FIXED_SELECTION
        let startup=AppModel();startup.crewdWindow.computers=[.init(id:remote)]
        await startup.connectCrewdAuthority()
        precondition(startup.crewdWindow.localSelections==1 && startup.crewdWindow.remoteSelections==0)
        let stable=startup.crewdWindow.workspace
        await startup.connectCrewdAuthority(computerID:UUID())
        precondition(startup.crewdWindow.workspace===stable && startup.crewdWindow.remoteSelections==0 && startup.crewdWindow.error != nil)
        startup.crewdWindow.computers=[.init(id:remote,paired:false)]
        await startup.connectCrewdAuthority(computerID:remote)
        precondition(startup.crewdWindow.workspace===stable && startup.crewdWindow.remoteSelections==0)
        startup.crewdWindow.computers=[.init(id:remote)]
        startup.crewdWindow.rejectSelection=true
        await startup.connectCrewdAuthority(computerID:remote)
        precondition(startup.crewdWindow.workspace===stable && startup.crewdBackend?.workspace===stable)
        print("PASS default startup local, missing/unpaired/invalid original record cannot bind replacement")
        #endif
    }
}
