# 当前交付：SwiftUI 精修 · 侧栏会话导航

主文件是 `PendingCrew-SwiftUI-Sidebar-Navigation.sketch`，不是此前 PrivateTeam 或 Refined / Final 文件。70 个原生可编辑画板，沿用原49个业务状态并迁移导航；新增5个边界、完整Codex控制与保留Tab的侧栏子页。完整列表在 `sidebar-navigation-profile.json`，实际校验在 `sidebar-native-integrity.json`。

## 最新导航裁决

左侧全高238pt，机组根单击选择，行尾向右箭头进入同一sidebar的「会话」子页。产品设计同时支持双击机组；Sketch播放正式入口为可点击箭头，不宣称原生原型能识别真实双击。子页里群聊与成员单聊混排，「‹ 机组」只返回侧栏根。消息、人待办、AI待办、文档、进展各有一份上下文对照，返回保留该Tab，不启动或恢复任务。后端机组有独立合成消息与会话子页。

旧独立196pt会话中列及18pt折叠边缘已经整体移除。旧14画板重用原稳定ID，改为「14 会话选择」；没有残留独立中列入口。右侧从sidebar右边铺满。五个连续玻璃Tab与单个圆形搜索在共同工作区最顶，Tab与内容间不再加Divider；消息工具行、待办与资料自身细线保留。新机组遵循实际pending分支：只有sidebar、single search、取消、5项metadata与同款单行Composer，不带旧群会话或五Tab。

## 贴合 SwiftUI 的尺度与组织

1280×820 Mac point画板，常规字体13/12/11，section标题17；28pt控件、30ptglass搜索、38pt附件/待办圆钮、30pt发送、44pt单行输入pill、7pt会话行圆角、18pt消息气泡。白色共用内容背景；sidebar浅中性材质；#E3EEEA只做局部选中，#044735低频用于明确主操作，危险操作使用来源Theme的terracotta。成员紧邻仅@你。Footer保稳定浅圆emoji、semibold机器名、向上chevron与同排状态点，机器菜单为中性色。

每个画板分成 Window、Sidebar、Workspace Toolbar、Content、Popovers & Sheets 原生组。Symbols页按 `SF-style/`、`Avatar/`、`Control/` 分组命名。图标、头像、按钮为真实SymbolInstance；按钮用原生label override。正文、标题、字段、矢量轮廓与样式都可编辑，不是截屏贴图。图标为SF语法的可编辑描绘，不冒称Apple原始SF Symbols路径。长内容用白色画布，材质只在导航、搜索、输入、原生浮层；sheet正文不让后面的消息穿透降低可读性。

## Codex 功能对照

以 `Sources/Mac/Crewd/CrewdCodexControlsView.swift`、`CrewdOriginalSessionControls.swift`、`CrewdCodexInputViews.swift` 为实际结构来源；不会把3个随意型号组合当实际目录。

| 原生页 | 设计状态与覆盖 |
| --- | --- |
| 概览 | 当前执行/队列/工作目录、期望与原生标题、同步标题、上下文/Token未知值、工具与Skills只读目录、明确手动恢复、能力与原请求核对 |
| 执行 | 原文、steer/queue分离输入、空闲时Steer与打断disabled、排入下一轮回执不代表投递/完成、停止整个会话、手动恢复、压缩、审查目标、原结构化请求答复 |
| 历史 | 读取不恢复、不回滚文件；轮次、停止状态分叉、归档/取消归档、永久删除独立不可逆确认、截断独立确认与不回滚文件语义 |
| 设置 | 已观察值与后续轮次草稿分离；实际模型目录未读取保持未知；model/effort独立；Fast与协作真实控件；审批策略/5类granular/审批者、原目录Profile与沙箱二选一、network、summary、后续轮次设置 |
| 输入与Skills | 当前/下一轮独立选择、只暂存不发送、图片/音频/UTF-8、拒绝PDF/任意二进制、原暂存丢弃/导出、本机选择重读、当前原目录Skills、Unknown只查原UUID |
| 边界 | 协议不可用显示disabled；模型目录/权限Profile/Skills不编造实际结果；恢复拒绝保原thread语义、Unknown保原提交，不自动重发或另开thread |

通用确认画板表达压缩、审查、归档和分叉的确认结构；永久删除与截断各有专用后果确认。输入原文、设置、排队、审批均只是合成预填状态，无模型或后台执行。

## 核验与可编辑边界

本机官方 sketchtool 2026.3.1 已读取当前新文件，全部70画板原生导出。schema、所有Symbol引用、ID唯一、目标与原生back连线、从唯一群聊起点到全部70画板可达均通过。`sidebar-previews/`保留12张代表性原生导出，完整导出在本地忽略目录`sidebar-exports/`，不将70张无谓产物全部打入提交。

当前侧栏版本没有做Sketch.app GUI播放。工具此前报告用户正在改变Sketch，按root要求停止GUI，保留PrivateTeam/Refined/Final窗口和可能的未保存编辑，没有为补验收另切窗口绕过保护。此前54态 Refined-Final 已实际点击新建与文档/Unknown/back/保存路线；这些历史GUI证据仅证明旧文件，不能冒充当前侧栏导航实播。`sidebar-verification-boundary.json`列出两层证据与准确hash。

Sketch播放使用预填画板与原生Flow/back模拟；不会自由键入、真搜索、真正持久化业务数据、连接机器、应用模型参数、替人审批或执行历史删除。浮层用完整保留背景状态模拟，不宣称独立overlay引擎。保存设计修改应直接保存自己的Sketch副本；重新运行生成器会覆盖生成文件，不覆盖用户打开文件中的未存设计。

## 重建

```sh
python3 refine_sidebar.py
python3 validate_refined.py PendingCrew-SwiftUI-Sidebar-Navigation.sketch
/Applications/Sketch.app/Contents/MacOS/sketchtool export artboards PendingCrew-SwiftUI-Sidebar-Navigation.sketch --output=sidebar-exports --formats=png --overwriting=YES --serial=YES
```

只写本日期原型目录。未改产品Swift/Rust/工程文件、真实DB、日用App、设备或Cloud；无commit/push。旧原型和所有先前候选继续保留。
