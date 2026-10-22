# 原生工作区收敛 — 用户最新裁定

本轮在已独验并封包的 d6a43a9cf6a986940900a6de7c5479e706d4103a 上修改。先前 c37/d6 的源码、包、原日志与 GUI 截图全部保留，不改标。本文件优先于 PLAN.md 中旧 demo 浮动侧栏/自绘内容切换/自绘搜索的外观建议。

用户明确要求：不用完全照搬 demo；sidebar 不需独立背景；消息与待办切换用 SwiftUI 原生并放整个会话版面上方；聊天无单独背景；当前群搜索用原生搜索框；已连接和当前未完成状态放 sidebar 底部；sidebar 整应用高度。根已接受，设计 addendum 与旧 d6 真实七图共同确定本轮必要范围，不扩大为新功能或验收夹具。

## 实际改动

- **整高原生侧栏**：移除 Mac 浮动圆角材质卡、白边、渐变和外边距；iOS 目录也恢复原生 sidebar list 背景。全宽顶部连接/错误条移除，侧栏不再被它向下挤。原系统隐藏/恢复、通讯录/消息选择和真实机组列表保留。
- **工作区顶层原生切换**：在 CrewdOriginalCrewContent 的最外层使用 SwiftUI segmented Picker，位于会话列表与正文共同上方，不跨全局机组 sidebar。消息/人待办/AI待办/文档数据/计划进展仍绑定原五个 section key；没有以 TabView 重建原聊天、草稿或订阅。
- **系统聊天底色与搜索**：聊天 canvas 改为系统 textBackgroundColor/systemBackground，不额外铺暖黑底；气泡、正文和选中态保留真实内容。Mac 改回 .searchable(.toolbar)，iPhone/iPad 的真实 IPadShell 也通过系统搜索框消费既有 CrewMessageSearch/时间线查询绑定，不扩搜索RPC或数据源，不宣称新全库查询。
- **一条会话工具行**：本群消息分类与管理进入原会话标题/沟通过滤行，不再独立大条。宽窗保 segmented，窄窗用带实际当前值的原生 menu。失效分类仍警示，分类选择与原 FolderStore/CAS/ACL/Unknown 逻辑未变。移除窗口工具菜单里重复的沟通范围/仅@你入口，当前会话上的原控件仍可用。
- **连接与原请求**：Mac 侧栏底部显示真实短状态，错误才警示；点击查看原状态/可展开完整错误文本、原 UUID/原请求只读核对。侧栏收起时工具栏只保小状态入口，不恢复常驻大条。iOS 全局 sidebar 底部同样有状态入口，侧栏折叠时从电脑/草稿菜单可达。没有清除 workspace.error/Unknown，没有自动重发或恢复；原手动恢复与管理确认仍由用户执行。紧凑入口不会重复触发首次异常退出提示。
- **内容优先的 inspector**：无运行机长的实际观察保留短行；执行器选择/能力/刷新折进“执行器准备”，新成员首任务与完整参数表单保持原入口，不再挤占右侧首屏。
- **成熟的两类待办**：主新建与次级关联/刷新一层组织，AI 原输入表单进入同一 sheet，仍使用原 binding、原 UUID 和原提交函数。没有事项时按人/AI解释真正可做的下一步；筛选空态可清搜索/标签。预设全0标签不形成一排主视觉，有数据及当前筛选保准确数。新建人工、人工答复/元数据、计划/分类和合并保存的主提交统一共用 primary；原 busy/Unknown/CAS 禁用条件逐页保留。
- **草稿动作**：本机保存/未保全文字继续常显；版本/导出收进一个次级菜单；保存错误、重试和原文保全仍可见，原 export ticket/CAS/退出 revision 闸没有改变。

新建机组首条任务页在旧实图中完整可读，没有重做。原一键创建机组+首消息后显式尝试机长的业务不改，不为测试添加纯 Human 开关。AI 待办 todo.create 实际会按原后端判断通知机长，所以新表单只说明“交给机组跟进”，不错误承诺永不触发会话。查看/切换/搜索/展开本轮 UI 本身不启动模型。

## 交通灯与系统弹窗事实

只读实际截图未见交通灯，不足以认定 App 故障。源里普通 WindowGroup；WindowSeparatorRemover 仅设置 separator.none、titlebar 透明与 fullSizeContentView，没有隐藏/自绘 standardWindowButton。本轮不因捕获/全屏可能性改系统窗装饰。原 NSAlert 退出与可关闭停工/force 语义不变，无继续后台选项。

## 验证和交接边界

旧 d6 GUI 证明旧候选的实际有内容布局问题；不能替代本修订 GUI 通过。设计一次补充为主仓私域 frontend-polish-design-20261022/visual-ruling-native-addendum.md；原图与 round1 报告保持原字节。

本轮最终暖 Mac Debug LOCAL_MAC+REMOTE_PRODUCT + ordinary Debug arm64 iOS Simulator full-target 均 exit0，388项前后输入相同；结果与输入在作者树私域 frontend-native-ruling-source-freeze-20261022/full-target-validation.json。所有准确 argv/log/hash保全；d6 normal 5d2307… 仅作为旧源准确编译输入，不冒新提交的最终 normal。前两轮已绿记录分别保 frontend-native-ruling-20261022 与 frontend-native-ruling-final-20261022，不覆写历史。

本轮不改 Rust/API/SQLite/schema/权限/设备身份/草稿存储/业务事务，不混 B f0/Cloud/APNs。未对日用 App、QA 当前数据/Unknown、设备、账号或模型做操作。根接受完整源码后再冻唯一完整输入、新准确 normal 和候选；本作者不先封旧 d6 新包。

新候选 GUI 需重点：Mac 原生整高 sidebar 与 footer、收起后的状态详情；原生 Picker 确实覆盖会话列表+正文共同顶部；900/1100/1440 的一行分类/过滤；真实 Unknown 原文与原 UUID 可达；人/AI真正空态/筛选/主提交；草稿错误保文；iPhone/iPad原生搜索/切换与折叠后状态。复用原稳定业务证据，不为新布局清理 Unknown 或重跑模型。
