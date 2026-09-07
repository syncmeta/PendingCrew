#if os(macOS)
import Foundation
import XCTest

/// 把「`$var` 后面直接跟多字节字符会炸」这条禁令的**前提**钉成可执行的用例。
///
/// 它测的不是我们的代码，是 `ReleaseScriptSourceContractTests` 那条静态禁令
/// **所依赖的那个机制**。三点理由：
///
/// 1. **它天生见过红** —— 第一条断言的就是「这个写法当场非零退出」，
///    不需要谁去人工制造一个坏例子再改回来。
/// 2. **哪天 bash / macOS 变了、这个写法不再咬人，它会第一个告诉我们。**
///    到那时才有资格谈退役那条禁令 —— 而不是靠某个人在错的 locale 里
///    复现失败，然后开始怀疑那把尺子。
/// 3. **它把 locale 这个条件钉在可执行的地方**，不只留在注释里。
///
/// ── 为什么必须写明 locale（2026-09-07 实测，`/bin/sh` = GNU bash 3.2.57 arm64-apple-darwin25）
///
///     LC_ALL=C            sh -uc 'v=1; echo "$v（x）"'    → rc=0   输出 `1（x）`
///     LC_ALL=en_US.UTF-8  sh -uc 'v=1; echo "$v（x）"'    → rc=127 输出 `/bin/sh: v?: unbound variable`
///     LC_ALL=en_US.UTF-8  sh -uc 'v=1; echo "${v}（x）"'  → rc=0   输出 `1（x）`  ← `${}` 确实是修法
///     LANG/LC_* 全未设     同第一条                        → rc=0
///
/// **在 `LC_CTYPE=C` 的环境里怎么试都是绿的。** 这一格真实咬过一次
/// （0.1.18 发版途中 tap 已推上去、脚本才非零退出），而复现它的条件是 locale，
/// 不是「哪台机器」。谁在错的 locale 里复现失败、然后判定那条禁令过时了，
/// 就会把一道真防线拆掉 —— 这个用例就是拦这件事的。
///
/// ── 一件它盖不住的事，写在这里免得下一个人以为有两道防线
///
/// `scripts/install.sh` 第 83 行（`say "→ 挂载并安装到 ${dest}…"`）**没有任何执行态保护**：
/// 那个脚本没有 `--help`、没有 dry-run，`set -eu` 之后一路查系统、拉 dmg、挂载、
/// 拷进 `/Applications`；能想到的早退路径**全都在第 83 行之前**，跑得过但根本碰不到
/// 出事的那一行。**静态那条禁令是它唯一的防线。** 谁想放宽字符集或口径，
/// 得先知道自己在拆的是什么。
final class ShellFullWidthPunctuationLocaleTests: XCTestCase {
    /// 跑一条 `sh -uc`，返回退出码。`locale` 为 nil 表示把 LC_ALL / LANG / LC_CTYPE 全清掉。
    private func runSh(_ script: String, locale: String?) throws -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-uc", script]
        var env = ProcessInfo.processInfo.environment
        env.removeValue(forKey: "LC_ALL")
        env.removeValue(forKey: "LANG")
        env.removeValue(forKey: "LC_CTYPE")
        if let locale { env["LC_ALL"] = locale }
        process.environment = env
        process.standardOutput = Pipe()
        process.standardError = Pipe()
        try process.run()
        process.waitUntilExit()
        return process.terminationStatus
    }

    func test_UTF8下裸变量紧跟全角标点会当场非零退出() throws {
        // 这一条断言的就是红。它红不了的那天，就是那条静态禁令该被重新评估的那天。
        XCTAssertNotEqual(
            try runSh(#"v=1; echo "$v（x）""#, locale: "en_US.UTF-8"), 0,
            "UTF-8 locale 下 `$v（` 本该以 unbound variable 非零退出。"
                + "如果这里绿了，说明 bash/macOS 的行为变了 —— "
                + "**先来更新这个用例和 ReleaseScriptSourceContractTests 的注释，别直接把禁令删了**"
        )
        // `…` 同样咬 —— 而它不在那把静态尺子的全角标点集里（`（）「」，。：；、`）。
        // install.sh:83 当初漏网就是因为这个字符。
        XCTAssertNotEqual(try runSh(#"v=1; echo "$v…""#, locale: "en_US.UTF-8"), 0)
    }

    func test_C_locale下同一个写法不炸_所以复现失败常常只是环境不对() throws {
        XCTAssertEqual(try runSh(#"v=1; echo "$v（x）""#, locale: "C"), 0)
        XCTAssertEqual(try runSh(#"v=1; echo "$v（x）""#, locale: nil), 0)
    }

    func test_花括号是修法_UTF8下也不炸() throws {
        XCTAssertEqual(try runSh(#"v=1; echo "${v}（x）""#, locale: "en_US.UTF-8"), 0)
        XCTAssertEqual(try runSh(#"v=1; echo "${v}…""#, locale: "en_US.UTF-8"), 0)
    }
}
#endif
