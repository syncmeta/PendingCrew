import Foundation
import XCTest

/// **这个 test bundle 是 standalone 的：源文件直接编进来，不 import app 模块。**
///
/// 规矩本身早就在，白纸黑字写在好几个测试文件的第二行：
/// 「CockpitTaskLedger.swift 在 Sources/Models（已整目录编进 test bundle），无需
/// @testable import」「不 @testable import —— PromptTemplate.swift 直接编进 test
/// bundle（见 project.yml）」。
///
/// ## 为什么要一条会红的断言，而不是把注释写得更大声
///
/// 2026-09-08 一天之内复发三次：早上 CLI 版本管理那单加了一处（`b73ac38` 删掉），
/// 紧接着有人把「这条规矩没人执行」写进了 tech-debt（`84f4f15`），当天又有人
/// 照着一份**落后于 main 的分支**读出「有 5 处」，据此推出一个**方向相反**的根因
/// （「给测试 target 加 `- target: PendingCrew` 依赖」——那会把 standalone 的
/// bundle 变成依赖 app 模块，正好走反）。
///
/// **三次都不是不守规矩，是不知道有这条规矩** —— 注释在那儿，但它在**别的文件**里，
/// 而写新测试的人不会去读别人的第二行。**人肉发现率实测是 0。**
///
/// ## 它红的时候是什么样
///
/// 加一句 `@testable import PendingCrew` 到任何一个测试文件，这条当场红并指名道姓。
/// 顺带它也解释了为什么该删而不是该加依赖：**冷 derivedDataPath 上，那句 import
/// 会让整趟测试挂在 `unable to resolve module 'PendingCrew'`** —— 因为测试 target
/// 本来就没有（也不该有）对 app target 的依赖。暖的 derivedData 上看不见这一点，
/// 所以它是一个只在别人机器上/CI 上炸的问题。
final class TestBundleStandaloneTests: XCTestCase {

    func test_测试目录下不许出现testable_import() throws {
        let testsDir = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()   // PendingCrewTests
            .deletingLastPathComponent()   // Tests
        let fm = FileManager.default
        guard let walker = fm.enumerator(at: testsDir, includingPropertiesForKeys: nil) else {
            return XCTFail("走不了 \(testsDir.path) —— 这条尺子这次什么都没量，别当它绿")
        }
        var offenders: [String] = []
        var scanned = 0
        for case let url as URL in walker where url.pathExtension == "swift" {
            scanned += 1
            let text = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
            // 只认**行首**的那句 import，不认散文里提到它的地方 ——
            // 第一版用 `contains`，结果把**这个文件自己**也标成了违规
            // （上面那段注释里逐字写着那句 import）。**一把会命中自己的尺子，
            // 第一次红的时候就会被当成误报关掉。**
            let offends = text.split(separator: "\n", omittingEmptySubsequences: false)
                .contains { $0.hasPrefix("@testable import PendingCrew") }
            if offends { offenders.append(url.lastPathComponent) }
        }
        // 先证明这把尺子真的扫到了东西：扫了 0 个文件的「全绿」跟真绿长得一样。
        XCTAssertGreaterThan(scanned, 50, "只扫到 \(scanned) 个 .swift —— 这趟根本没量到东西")
        XCTAssertEqual(offenders.sorted(), [],
                       "这几个文件 `@testable import PendingCrew` 了。**删掉那一行**，"
                       + "别给测试 target 加 `- target: PendingCrew` 依赖 —— "
                       + "这个 bundle 是 standalone 的，源文件直接编进来（见 project.yml），"
                       + "加依赖是往反方向走。")
    }
}
