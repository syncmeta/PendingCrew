import Foundation

/// 把扁平 crew 列表按所属机器分组，给 Mac 侧栏「按机器组织」用。
/// 纯函数、平台无关、可单测 —— 不碰 store / SwiftUI。
enum MachineGrouping {
    /// 一个机器分组。`machine == nil` 是「其它/解析不到」兜底桶。
    struct Group: Identifiable, Equatable {
        let machine: Machine?
        let crews: [CrewSummary]
        var id: String { machine?.id ?? "__other__" }
    }

    /// - crews: 当前可见 crew（Mac = 本地本机 crew）。
    /// - machines: 合并后的机器清单（本机 + 账号其它机器）。
    /// - localDeviceId: `DeviceIdentity.current`，用来认「本机」那台。
    ///
    /// 规则：
    /// - `crew.machineId == nil` → 归本机（localDeviceId 对应的机器；无本机则落其它桶）。
    /// - `crew.machineId == 某 machine.id` → 该机器；指向本机 deviceId/id 也认本机。
    /// - 都不匹配 → 末尾「其它」桶（仅非空时出现）。
    /// - 机器顺序：本机置顶 → 其它 computer → fly → 未知 kind；同级按 displayName。
    /// - 空机器也出组（含本机空）。
    static func group(
        crews: [CrewSummary],
        machines: [Machine],
        localDeviceId: String
    ) -> [Group] {
        let localMachine = machines.first { $0.deviceId == localDeviceId }
        let byId = Dictionary(machines.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })

        func resolve(_ crew: CrewSummary) -> String? {
            if let mid = crew.machineId {
                if byId[mid] != nil { return mid }
                if let local = localMachine, mid == local.deviceId || mid == local.id {
                    return local.id
                }
                return nil
            }
            return localMachine?.id
        }

        var bucket: [String: [CrewSummary]] = [:]
        var other: [CrewSummary] = []
        for crew in crews {
            if let mid = resolve(crew) { bucket[mid, default: []].append(crew) }
            else { other.append(crew) }
        }

        func rank(_ m: Machine) -> Int {
            if m.deviceId == localDeviceId { return 0 }
            switch m.kindEnum {
            case .computer: return 1
            case .fly: return 2
            case .none: return 3
            }
        }
        let ordered = machines.sorted { lhs, rhs in
            let (rl, rr) = (rank(lhs), rank(rhs))
            if rl != rr { return rl < rr }
            return lhs.displayName.localizedCompare(rhs.displayName) == .orderedAscending
        }

        var groups = ordered.map { Group(machine: $0, crews: bucket[$0.id] ?? []) }
        if !other.isEmpty { groups.append(Group(machine: nil, crews: other)) }
        return groups
    }
}
