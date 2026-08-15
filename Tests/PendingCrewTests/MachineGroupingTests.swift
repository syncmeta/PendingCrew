import XCTest
import Foundation

final class MachineGroupingTests: XCTestCase {

    // MARK: - helpers
    private func machine(_ id: String, device: String?, kind: String = "computer",
                         name: String = "M") -> Machine {
        Machine(id: id, kind: kind, deviceId: device, displayName: name,
                flyMachineId: nil, status: "online", lastSeenAt: nil)
    }
    private func crew(_ id: String, machineId: String?) -> CrewSummary {
        CrewSummary(id: id, title: id, responsibleSubjectId: "s",
                    runtimeLocation: "local_host", captainBotId: nil, status: nil,
                    createdAt: "", updatedAt: "", parentCrewIds: [],
                    captainAgentKind: nil, machineId: machineId)
    }

    func testNilMachineIdGoesToLocal() {
        let local = machine("m-local", device: "dev-1", name: "本机")
        let groups = MachineGrouping.group(
            crews: [crew("c1", machineId: nil)],
            machines: [local], localDeviceId: "dev-1")
        XCTAssertEqual(groups.count, 1)
        XCTAssertEqual(groups[0].machine?.id, "m-local")
        XCTAssertEqual(groups[0].crews.map(\.id), ["c1"])
    }

    func testMatchingMachineIdRoutesToThatMachine() {
        let local = machine("m-local", device: "dev-1", name: "本机")
        let peer = machine("m-peer", device: "dev-2", name: "Peer")
        let groups = MachineGrouping.group(
            crews: [crew("c1", machineId: "m-peer")],
            machines: [local, peer], localDeviceId: "dev-1")
        let peerGroup = groups.first { $0.machine?.id == "m-peer" }
        XCTAssertEqual(peerGroup?.crews.map(\.id), ["c1"])
    }

    func testCrewMachineIdEqualToLocalDeviceIdResolvesToLocal() {
        // 本地合成机器 id == deviceId 的历史路径：crew.machineId 写成 deviceId 也认本机。
        let local = machine("dev-1", device: "dev-1", name: "本机")
        let groups = MachineGrouping.group(
            crews: [crew("c1", machineId: "dev-1")],
            machines: [local], localDeviceId: "dev-1")
        XCTAssertEqual(groups.first?.crews.map(\.id), ["c1"])
    }

    func testEmptyMachineStillAppearsAsGroup() {
        let local = machine("m-local", device: "dev-1", name: "本机")
        let peer = machine("m-peer", device: "dev-2", name: "Peer")
        let groups = MachineGrouping.group(
            crews: [crew("c1", machineId: nil)],
            machines: [local, peer], localDeviceId: "dev-1")
        XCTAssertEqual(groups.count, 2)
        let peerGroup = groups.first { $0.machine?.id == "m-peer" }
        XCTAssertNotNil(peerGroup)
        XCTAssertTrue(peerGroup!.crews.isEmpty)
    }

    func testLocalMachineSortsFirst() {
        let local = machine("m-local", device: "dev-1", name: "ZZZ本机")
        let peer = machine("m-peer", device: "dev-2", name: "AAA")
        let fly = machine("m-fly", device: nil, kind: "fly", name: "fly")
        let groups = MachineGrouping.group(
            crews: [], machines: [fly, peer, local], localDeviceId: "dev-1")
        XCTAssertEqual(groups.map { $0.machine?.id }, ["m-local", "m-peer", "m-fly"])
    }

    func testUnresolvedCrewGoesToOtherBucket() {
        let local = machine("m-local", device: "dev-1", name: "本机")
        let groups = MachineGrouping.group(
            crews: [crew("c1", machineId: "ghost")],
            machines: [local], localDeviceId: "dev-1")
        let other = groups.last
        XCTAssertNil(other?.machine)
        XCTAssertEqual(other?.crews.map(\.id), ["c1"])
    }

    func testNoOtherBucketWhenAllResolved() {
        let local = machine("m-local", device: "dev-1", name: "本机")
        let groups = MachineGrouping.group(
            crews: [crew("c1", machineId: nil)],
            machines: [local], localDeviceId: "dev-1")
        XCTAssertFalse(groups.contains { $0.machine == nil })
    }
}
