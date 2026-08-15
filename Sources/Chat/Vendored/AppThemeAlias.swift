import SwiftUI

/// 应用 `Theme` 的**不带模块限定**的别名。
///
/// `MarkdownText.swift` 同时 `import MarkdownUI`，那边也有一个 `Theme`，所以在那个
/// 文件里裸写 `Theme` 是歧义的。原来靠 `PendingCrew.Theme` 用模块名消歧 —— 但那把
/// 文件钉死在了「模块必须叫 PendingCrew」上：`CrewChatOpenCostTests` 要把这条 markdown
/// 渲染链直编进 `PendingCrewTests` bundle 去量真实成本，模块名一变就编不过。
///
/// 这个文件**不 import MarkdownUI**，所以这里的 `Theme` 无歧义；下游用别名即可，
/// 与模块名解耦。
typealias AppThemeAlias = Theme
