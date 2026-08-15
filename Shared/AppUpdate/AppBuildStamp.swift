import Foundation

/// 读构建戳（`stamp-build-info.sh` 写进 Info.plist 的 BuildStampCommit/Date），
/// 拼「当前版本」显示串。PendingBot / PendingCrew 共用一份（apps/shared）。
///
/// 构建没打戳（比如没装 git 的机器上编的）就明说「无版本戳」——
/// 不写占位 SHA 假装能比对。
enum AppBuildStamp {
    static func versionDisplay(info: [String: Any]) -> String {
        let version = info["CFBundleShortVersionString"] as? String ?? "?"
        let build = info["CFBundleVersion"] as? String ?? "?"
        guard let commit = info["BuildStampCommit"] as? String, commit.count >= 7 else {
            return "\(version) (\(build)) · 无版本戳"
        }
        return "\(version) (\(build)) · \(commit.prefix(7))"
    }

    static var current: String {
        versionDisplay(info: Bundle.main.infoDictionary ?? [:])
    }
}
