import Foundation

enum CrewdRetirementRecord {
    static func decode(_ value: CrewdValue, crew: String, session: String) throws -> CrewdValue {
        guard value["crewId"].string == crew, value["sessionId"].string == session,
              case .object(let fields) = value, let record = fields["record"] else {
            throw CrewdClientError(message: "退场记录不属于当前成员或不完整。")
        }
        if record == .null { return record }
        guard case .object(let row) = record,
              row["decisionId"]?.string?.isEmpty == false,
              let generation = row["generation"]?.string, let number = UInt64(generation), String(number) == generation,
              let phase = row["phase"]?.string,
              ["prepared","dispatched","decision_ready","exit_requested","parked","stale","unknown","suppressed"].contains(phase),
              let reason = row["reason"]?.string,
              ["prepared","answered","low_confidence","provider_unavailable","unconfigured","restart_unknown","state_changed","stopped","exit_unknown"].contains(reason),
              case .bool? = row["adopted"], let probability = row["probability"] else {
            throw CrewdClientError(message: "退场记录格式无效。")
        }
        if probability != .null {
            let number: Double?
            switch probability { case .number(let n): number=n; case .integer(let n): number=Double(n); case .unsigned(let n): number=Double(n); default: number=nil }
            guard let number, number.isFinite, (0...1).contains(number) else { throw CrewdClientError(message:"退场判断数值无效。") }
        }
        return record
    }
}
