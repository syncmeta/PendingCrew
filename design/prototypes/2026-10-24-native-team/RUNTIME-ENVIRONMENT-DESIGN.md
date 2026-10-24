# Sidebar 与运行环境：最终静态视觉同步

文件：`PendingCrew-SwiftUI-Runtime-Environment.sketch`。SHA-256：`646b7d41046af2cfadd6757859e3160bf060dc7727c6e668dd9757f9094cf8b3`。

## 对照与范围

客户端视觉源码冻结：`86c4ac2e0cc808c61692281d074c88b7060fea23fbc081fd0f865061036ccc5f`，基线 `2dbc9256`。尺度来源为 `docs/private/preserved-delivery-artifacts/sidebar-visual-unification-20261024/VISUAL-SCOPE.md`、`Sources/Mac/Crewd/CrewdHumanActions.swift` 的 `CrewSidebarRowStyle` / `CrewSidebarRowSurface`、`CrewdOriginalConnectionIntegration.swift` 的原生 NSMenu footer 和 `CrewSidebarView.swift`。

仅 Sidebar 同步：机组与会话共用 52pt 基础行、横10/纵8 inset、内容最小36、行距6、行内4、圆角14；标题13 semibold、副文12、时间11。机组加36头像，会话不画头像。两个列表保留原路由、单击选择、机组行尾箭头进入会话子页、返回保留 Tab；Sketch 不模拟真实双击。Sidebar 以同一 helper 生成全部70态。

footer 保留26浅圆 emoji、13 semibold 名称；状态在名称左，up chevron 缩至10。菜单向上展开，标题「运行环境」，中间仅运行环境名称，底部分隔后「运行环境设置」。该动作进入现有设置状态，原机组元数据「机器」不擅自改名。原退出状态仍可通过停工完成→退出进入，不再塞进运行环境列表。现有连接设置可从「运行环境与连接」到达；不增加任何后台能力。

右侧沿既有70态内容：共同顶部五Tab/唯一搜索、无 Tab 下分隔线、新建 pending 分支、成员/群聊、待办、知识、进展和 Codex 四页均保留。本轮不重构右侧成员/群聊壳。

## 图层、流程与证据

复用 `refine_sidebar.py --runtime`，没有复制一套生成器。70态 artboard ID 与旧交付一一相同；162 个 native Symbols、2104 个实例、2360 个 Flow、44 个 back，单 home 全70可达。所有正文为可编辑文字和矢量，组件位于 `Native Components`；层组沿用 Window / Sidebar / Workspace Toolbar / Content / Popovers & Sheets。

`runtime-environment-integrity.json` 是官方 schema、唯一ID、Symbol引用和完整 Flow 检查；`runtime-verification-boundary.json` 记录旧交付 hash 不变、70个画板ID保留以及未操作 GUI。CLI 实际导出70/70，`runtime-previews/` 保留11张代表图，完整导出仅在本地忽略目录 `runtime-exports/`。旧 Sketch 与旧证据未覆盖。另对01/07/14/15/21/38/62/70的右侧区域 x≥238 进行新旧 CLI PNG 像素比较，八张均完全一致。

本版是原生设计状态：点击到预填前后态，播放不能自由键入或执行模型/后端，需在 Sketch 编辑文字。浮层用完整画板及 Flow/back 表达。尚无本版 Sketch GUI 播放；因保护用户正在编辑的窗口，严格无 GUI。静态视觉冻结不等于客户端 Swift 类型构建或运行验收；旧版 native 播放不挪用到本版。

重新生成前先保留人工修改；仅在确定新目标可覆盖时执行：

```sh
python3 -B refine_sidebar.py --runtime
python3 -B validate_refined.py PendingCrew-SwiftUI-Runtime-Environment.sketch
/Applications/Sketch.app/Contents/MacOS/sketchtool export artboards PendingCrew-SwiftUI-Runtime-Environment.sketch --output=runtime-exports --formats=png --overwriting=YES --serial=YES
```
