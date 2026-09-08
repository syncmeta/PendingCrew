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

    /// `$var` 后面直接跟**非 ASCII 字符**时，`/bin/sh`（macOS 上是 bash 3.2）会把那个
    /// 多字节字符的头一个字节算进变量名，于是 `set -u` 下当场 `unbound variable`。
    ///
    /// ## 触发条件是 locale，不是机器（2026-09-07 补）
    /// **只有 `LC_CTYPE` 是 UTF-8 时才咬。**同一台机器、同一个 `/bin/sh`
    /// （bash 3.2.57）实测三行：
    /// ```
    /// （LANG 未设，agent shell 的默认）  sh -uc 'v=1; echo "$v（x）"'  →  1（x）   exit 0
    /// LC_ALL=C                          同上                          →  1（x）   exit 0
    /// LC_ALL=en_US.UTF-8                同上                          →  sh: v?: unbound variable   exit 127
    /// ```
    /// 这一条**必须写在这儿**：agent 的 shell 通常没有 UTF-8 locale，于是**谁去复现都是绿的**，
    /// 很容易得出「这把尺子在防一件没发生过的事」而把它拆掉。2026-09-07 就差点发生
    /// （复现命令原本一直躺在 `d5a5f6e` 的 commit message 里，只是没人回去找）。
    /// 它也解释了为什么偏偏咬发版脚本：那种环境几乎必然是 UTF-8。
    ///
    /// **这张表在仓库里有两份**：这一份是静态文本，会随 bash / macOS 变化而悄悄过期；
    /// 另一份是 `ShellFullWidthPunctuationLocaleTests` 里的断言，**过期那天它会红**。
    /// **两份打架时以断言那份为准。** 这句话写在这儿而不是写在断言里，是因为
    /// 会过期的是这一份，而读到这一份的人正是需要被提醒的那个。
    /// 知道它有两份，比假装只有一份安全 —— 别为了「消除重复」删掉任何一份：
    /// 一份要给人读、一份要给机器跑，**要紧的不是消除重复，是声明谁说了算，
    /// 而答案永远是那个会自己报警的。**
    ///
    /// ## 判据是「非 ASCII」，不是一张全角标点表（2026-09-07 改）
    /// 原本枚举的是 `（）「」，。：；、`，于是 `scripts/install.sh:83` 的 `$dest…` 从来
    /// 没被看见过 —— `…` 不在表里，而它在 UTF-8 下照样红。那是 `curl | sh` 给真人跑的
    /// 安装脚本、脚本头就是 `set -eu`，真人的终端几乎必然是 UTF-8：**那一行会让安装在
    /// 「挂载并安装到…」当场以 127 死掉，而我们这边永远复现不出来。**
    /// 名单会短，字节不会 —— 所以判据改成「裸 `$var` 后紧跟任意非 ASCII」。
    /// **不扩到 `${var}` 那一侧**：那是修法本身，扩过去就是过报，而过报的尺子会被人关掉。
    ///
    /// 这不是理论问题，是 2026-08-27 发 0.1.18 时**在发布途中**咬了一口：
    /// `update-homebrew-tap.sh` 最后那句成功回执写的是 `"...已更新到 $version（$tap_repo）"`，
    /// 于是脚本**在 tap 已经推上去之后**才以非零码退出 —— 活干完了，回执说失败。
    /// 这个方向比「没干活还报成功」更阴：调用方会去重试或当它没发生。
    ///
    /// 而且它**已经复发过一次**：`fedb697` 只修了 feed 那一处，仓库里当时还剩 24 处。
    /// 所以这里不是修一处，是把整类钉住。修法：`${var}` 显式括起来。
    func testShellScriptsBraceVariablesBeforeNonASCII() throws {
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
                    // **注释行跳过**。2026-09-08：main 上 `scripts/shell-var-brace-check.sh`
                    // 是**讲这同一条规则**的脚本，它的注释里必须写出 `$dpid，` / `$dest…`
                    // 这种坏例子才讲得清楚 —— 于是这把尺子把「规则的说明书」判成了违规，
                    // 全量当场红，而那两行根本不会被执行。
                    // 判据零判断：**整行第一个非空字符是 `#`** 就是注释。不做「行内 #
                    // 之后算注释」那种事 —— 字符串里的 `#` 不是注释，那会开始漏报。
                    guard !line.trimmingCharacters(in: .whitespaces).hasPrefix("#") else { continue }
                    for match in Self.badVariableUses(in: line) {
                        offenders.append("\(relative):\(index + 1): \(match)")
                    }
                }
            }
        }

        XCTAssertEqual(
            offenders, [],
            "这些 $var 紧跟非 ASCII 字符，sh 会把那个字符的首字节读进变量名（UTF-8 locale 下 set -u 当场 unbound variable）—— 改成 ${var}："
                + offenders.joined(separator: " | ")
        )
    }

    /// 找 `$name` / `$1` / `$@` 之类后面紧跟**任意非 ASCII 字符**的写法。
    ///
    /// 判的是字节而不是一张字符表：会咬人的是「多字节字符的头一个字节被算进变量名」，
    /// 跟那个字符具体是什么无关。列表会短（`…` 就漏过去了），字节不会。
    ///
    /// ## 一条给以后改这个扫描器的人
    /// **扫描器对某个概念的定义，必须去对齐被扫对象的定义，而不是自己发明一个更聪明的。**
    /// 这里的「变量名」跟着 shell 走（只有 `[A-Za-z0-9_]`），不跟着 Swift 的
    /// `Character.isLetter` 走 —— 后者是 Unicode 感知的，比 shell 宽，于是它认得出
    /// `$dest`、却会被 `目录` 带着一路吃到行尾。同一晚上这个形状撞了三次
    /// （字符集短一个 `…`、文档行号对着一棵没写下来的树、这里的变量名），
    /// **三次都不是逻辑错，是对象错，而且三次都通过了自己的测试** —— 因为测试用的
    /// 是同一个定义。
    private static func badVariableUses(in line: String) -> [String] {
        var found: [String] = []
        let chars = Array(line)
        var i = 0
        while i < chars.count {
            guard chars[i] == "$" else { i += 1; continue }
            var j = i + 1
            guard j < chars.count else { break }
            // 变量名只认 ASCII。`Character.isLetter` / `.isNumber` 是 **Unicode 感知**的：
            // 不钉住的话 `$dest目录` 里的「目录」会被当成变量名的一部分吃掉，扫到结尾
            // 也就没有「后面紧跟非 ASCII」可判了 —— 一条真会咬人的写法就这么静默漏过。
            // shell 的变量名本来就只有 [A-Za-z0-9_]，这里跟着它。
            if chars[j].isASCII, chars[j].isLetter || chars[j] == "_" {
                while j < chars.count, chars[j].isASCII,
                      chars[j].isLetter || chars[j].isNumber || chars[j] == "_" { j += 1 }
            } else if chars[j].isASCII, chars[j].isNumber || "?@*#!$".contains(chars[j]) {
                j += 1
            } else {
                i += 1
                continue
            }
            if j < chars.count, !chars[j].isASCII {
                found.append(String(chars[i..<min(j + 1, chars.count)]))
            }
            i = j
        }
        return found
    }
}
