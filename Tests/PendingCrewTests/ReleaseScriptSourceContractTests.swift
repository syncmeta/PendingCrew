import Foundation
import XCTest

final class ReleaseScriptSourceContractTests: XCTestCase {
    func testMacReleaseSnapshotAndTagUseTheSameResolvedRef() throws {
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // PendingCrewTests
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // repo root
        let scriptURL = repoRoot
            .appendingPathComponent("scripts/release/build-macos-update.sh")
        let script = try String(contentsOf: scriptURL, encoding: .utf8)

        XCTAssertTrue(script.contains("release_ref=${1:-main}"))
        XCTAssertTrue(script.contains("rev-parse --verify \"$release_ref^{commit}\""))
        XCTAssertTrue(script.contains("worktree add --detach \"$snap/src\" \"$release_commit\""))
        XCTAssertTrue(script.contains("snapshot_commit=$(git -C \"$snap/src\" rev-parse HEAD)"))
        XCTAssertTrue(script.contains("[ \"$snapshot_commit\" = \"$release_commit\" ]"))
        XCTAssertTrue(script.contains("\"$snap/src/CHANGELOG.md\""))
        XCTAssertTrue(script.contains("tag \"$tag_name\" \"$snapshot_commit\""))
        XCTAssertTrue(script.contains("[ \"$tagged_commit\" != \"$snapshot_commit\" ]"))

        XCTAssertFalse(script.contains("worktree add --detach \"$snap/src\" main"))
        XCTAssertFalse(script.contains("tag \"v$version\" main"))
    }

    /// `$var` 后面直接跟全角标点时，`/bin/sh`（macOS 上是 bash 3.2）会把那个多字节
    /// 字符的头一个字节算进变量名，于是 `set -u` 下当场 `unbound variable`。
    ///
    /// 这不是理论问题，是 2026-08-27 发 0.1.18 时**在发布途中**咬了一口：
    /// `update-homebrew-tap.sh` 最后那句成功回执写的是 `"...已更新到 $version（$tap_repo）"`，
    /// 于是脚本**在 tap 已经推上去之后**才以非零码退出 —— 活干完了，回执说失败。
    /// 这个方向比「没干活还报成功」更阴：调用方会去重试或当它没发生。
    ///
    /// 而且它**已经复发过一次**：`fedb697` 只修了 feed 那一处，仓库里当时还剩 24 处。
    /// 所以这里不是修一处，是把整类钉住。修法：`${var}` 显式括起来。
    func testShellScriptsBraceVariablesBeforeFullWidthPunctuation() throws {
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let fm = FileManager.default
        var offenders: [String] = []

        for dir in ["scripts", "Shared/scripts"] {
            let root = repoRoot.appendingPathComponent(dir)
            guard let walker = fm.enumerator(at: root, includingPropertiesForKeys: nil) else {
                XCTFail("枚举不了 \(dir)")
                continue
            }
            for case let url as URL in walker where url.pathExtension == "sh" {
                let text = try String(contentsOf: url, encoding: .utf8)
                // 路径要能直接点开：用相对仓库根的真实路径，别拿 dir + 文件名拼 ——
                // scripts/release/x.sh 会被拼成 scripts/x.sh，指到一个不存在的地方。
                let relative = url.path.replacingOccurrences(
                    of: repoRoot.path + "/", with: ""
                )
                for (index, line) in text.components(separatedBy: "\n").enumerated() {
                    for match in Self.badVariableUses(in: line) {
                        offenders.append("\(relative):\(index + 1): \(match)")
                    }
                }
            }
        }

        XCTAssertEqual(
            offenders, [],
            "这些 $var 紧跟全角标点，sh 会把标点的首字节读进变量名 —— 改成 ${var}："
                + offenders.joined(separator: " | ")
        )
    }

    /// 找 `$name` / `$1` / `$@` 之类后面紧跟全角标点的写法。
    private static func badVariableUses(in line: String) -> [String] {
        let fullWidth = Set("（）「」，。：；、")
        var found: [String] = []
        let chars = Array(line)
        var i = 0
        while i < chars.count {
            guard chars[i] == "$" else { i += 1; continue }
            var j = i + 1
            guard j < chars.count else { break }
            if chars[j].isLetter || chars[j] == "_" {
                while j < chars.count, chars[j].isLetter || chars[j].isNumber || chars[j] == "_" { j += 1 }
            } else if chars[j].isNumber || "?@*#!$".contains(chars[j]) {
                j += 1
            } else {
                i += 1
                continue
            }
            if j < chars.count, fullWidth.contains(chars[j]) {
                found.append(String(chars[i..<min(j + 1, chars.count)]))
            }
            i = j
        }
        return found
    }
}
