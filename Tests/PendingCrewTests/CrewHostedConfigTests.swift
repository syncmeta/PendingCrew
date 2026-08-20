import XCTest
// CrewHostedConfig.swift 直接编进 PendingCrewTests target（纯 Foundation）。

/// `CrewHostedConfig.isConfigured` 是「云端这条线通不通」的唯一真值 ——
/// README 的能力清单和三条登录入口都引用它，所以它必须真的会变。
final class CrewHostedConfigTests: XCTestCase {
    /// 开源仓库里这四个常量必须还是占位值。
    ///
    /// 这条同时是一道**泄密闸**：谁哪天顺手把自己真实的 Supabase 项目坐标
    /// 提交进来，这里会红。
    func testRepoShipsPlaceholdersOnly() {
        XCTAssertFalse(
            CrewHostedConfig.isConfigured,
            "仓库里不该带真实后端坐标 —— 要么是有人提交了自己的项目坐标，要么是占位判据写坏了")
        XCTAssertTrue(CrewHostedConfig.supabaseURL.absoluteString.contains("YOUR-PROJECT"))
        XCTAssertTrue(CrewHostedConfig.supabasePublishableKey.contains("REPLACE_ME"))
    }

    /// 给用户看的说明必须先讲「本地 crew 不需要它」—— 大多数人到这儿是误入，
    /// 不能让他以为自己错过了什么功能。
    func testUnconfiguredNoticeTellsUserLocalCrewIsUnaffected() {
        let notice = CrewHostedConfig.unconfiguredNotice
        XCTAssertTrue(notice.contains("本地 crew"))
        XCTAssertTrue(notice.contains("不需要"))
        XCTAssertFalse(notice.isEmpty)
    }
}
