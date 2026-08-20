## 这个 PR 干什么

<!-- 一两句。**说为什么**，不只说改了什么。 -->

## 你跑过什么

<!-- 贴命令和结果。「应该没问题」不算验证。 -->

- [ ] `xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' build`
- [ ] `xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' test`
- [ ] `xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'generic/platform=iOS Simulator' build`

## 检查项

- [ ] 改了 `project.yml`（**包括新增 Swift 文件**）→ 跑过 `xcodegen`，`.pbxproj` 一起提交了
- [ ] 如果为了让 A 跑通而把代价转嫁给了 B → 在 `docs/tech-debt.md` 记了一条，或加了一道会响的断言
- [ ] 没夹带无关的格式化改动
