# 完整原生体验打磨方案 — 已接受执行

产品基线为已安装并验收的 main `c78c9b6ddb49db999f054e74cd3b248760906cbd`；最新三项发布截图/证据已普通rebase至 `5d32b32e021e99477e1950581b4320293b892ec7`，独立分支 `codex/mac-demo-polish-20261022`。B 全部保存于原分支 `f0a81c20`，不进入本候选；Cloud 账号/APNs/未冻结后台 API 同样不进入。本模块只修改原生产品的视觉、信息组织与交互表达，现 Core/SQLite/ACL/receipt/草稿与设备身份保持真实。

Demo 精确目录 `design/prototypes/2026-10-14-react-product-tour`，当前实际 TSX/CSS/资产与 primary 全部逐 hash 一致，逐文件证据在 `DEMO-BASELINE.json`。以 `PendingCrewTour.tsx`、`Sidebar/ConversationList/Chat/Inspector/Workspace/Dialogs.tsx` 与 `styles/pendingcrew.css` 的最终级联规则为准：尤其 CSS 最后追加的浮动半透明侧栏、去装饰 lineage、两种待办计数、会话角标、五个顶级内容入口与胶囊输入区，不只套最开始的浅色变量。样本展示值、假时刻、模拟额度、模拟更新与正文替换函数不进入真实产品。

## 当前实际入口与根因

实际 Mac 入口是 `PendingCrewApp → MacThreePaneView → CrewSidebarView/CrewCenterView/CrewChatView`，经 `CrewdOriginalWorkIntegration` 加入本群内容，经 `CrewSessionWindowView` 加入右侧会话/待办。iOS 为 `IPadShell → CrewListView/CrewChatView + 同一 WorkIntegration`。不去打磨未被日常产品采用的备用 `CrewdWorkspaceView` 当成完成。

现在的主要缺口是原聊天与新业务面板各有一套间距、灰度、信息密度和按钮层级；新表单把底层字段/JSON、能力说明和主操作平铺在同一层；导航、群工具和右栏相互争夺空间。单纯改色或给 GroupBox 加圆角无法解决。需要统一布局骨架、内容层级、交互节奏与完整状态表达。

## 一套设计系统

在既有 `CrewdChatStyle` 上收敛跨端原语，增加少量共享 presentation 源，不另造业务 Store：

- 颜色与层级：demo 深绿主操作、暖灰背景、细分隔、安静选中态；Mac 浮动材质侧栏与极淡冷暖底色，正文白底；深色模式使用同一语义层级，不能硬贴白色截图。
- 字体与密度：Mac 正文13、标题15/600、二级11–12，单一 mono 只用于可展开身份/诊断；iOS 按 Dynamic Type 与触控尺寸适配，不能把桌面13点文字和28点小按钮硬搬到手机。
- 原语：图标工具按钮/主要与次要动作、轻标签/状态点/计数、分组区段与表单行、统一 header/footer 的 sheet、空/载入/出错/离线面板、身份与只读提示、可展开技术明细、操作进行中反馈。已有结构化审批 Form 与知识 renderer 继续复用。
- 焦点、hover、按下、禁用与忙碌都有可见差异；图标有 help/accessibilityLabel，颜色不单独承担状态；支持键盘 Tab/Return/Escape、Reduce Motion 与正文选择复制。

## 页面与交互整体映射

| 产品面 | Demo依据 | 本轮原生落地与完成标准 |
| --- | --- | --- |
| 导航与通讯录/消息 | Sidebar/ConversationList/QuotaFooter | 统一侧栏工具区与模式切换；层级靠缩进/展开箭头；机组行有真实更新时间与可用的真实待办计数；会话行固定头像、标题、时间、摘要与选中节奏。未读取计数不冒0，未知额度不画假进度。 |
| 主顶栏与内容入口 | PendingCrewTour | 机组标题+真实电脑/当前群路径，搜索和常用工具有稳定位置；消息/人待办/AI待办/文档数据/计划进展五入口同一套选中样式，独立查询/标签状态继续沿原实现。群与全库范围明确。 |
| 群消息与输入 | Chat/MessageText/Inspector | 收敛发送者、时间、气泡、引用卡片与连续消息间距；保原消息分页/滚动锚点/草稿；通知成员、发消息/建待办主动作清楚分层；附件或其他未接能力如实呈现，不补假按钮；原用户文本逐字保留。 |
| 右侧待办/成员 | Inspector | 同一群的真实未完成待办摘要、成员与运行状态；正文/状态/负责人/标签分清；会话终端控制从成员条目明确进入，不把面板全部摊开。保持同一实例，收放不会重连或恢复会话。 |
| 人与AI待办 | Workspace/TodoOwner/TodoTags | 用工作流列表替代密集表单堆叠：标题、原状态、负责人、多标签和来源在不同层级，主操作就地，编辑在统一详情；Human答复与手动done不混，Unknown只核对原请求；群引用和源全局属性仍按真实合同。 |
| 文档/知识 | Workspace/DocumentContent/DataViewPanel | 库/本群引用范围清楚，列表、分类、正文阅读与工具层次统一；沿现 View/Block/Placement与自由Web renderer，原保存→加入群两阶段身份保留；不创建固定知识模板，不回旧document表。 |
| 计划/进展 | PlanTable | 宽窗层级表，窄窗同信息分层行；状态、负责人、依赖/卡点、下一检查可读；真实层级可展开，长标题不撑坏布局，不把派生状态冒手动完成。 |
| 新建机组/成员 | Dialogs | 第一步突出名称、主机目录与具体首任务；模型/后端/审批是有说明的次级设置；校验就地、底部主按钮固定，原任务/显式标题优先；提交期间反馈和失败原文保留。 |
| Codex会话 | 原生已接完整控制能力 | 从“开关和JSON堆”整理成会话概览、任务输入与队列、模型/权限、历史、更多控制；实际/期望/未确认状态用可读字段呈现，原JSON仅可展开诊断；标题、usage与上下文、停止/steer/review/fork等现能力入口完整保留，未接线与供方不支持分开。 |
| 连接/配对与原请求 | 现稳定路径+统一表单 | 电脑身份、指纹、有效期/状态分层；主操作集中，旧稳定文件配对不改协议或私钥/授信规则；本模块不带入B单链接半成品。离线缓存/草稿与不可执行状态明确，错误带正确下一步。 |
| 退出/停工 | 原有quit协调 | 统一标题、简短后果说明、直接退出/安全暂停/叫停选择与按钮层级；停工进度可关闭、force语义不变；未耐久原文保文/导出/确认退出仍走原精确revision闸。 |

## 窗口与跨端布局

Mac 以现原生导航根为基础，将主侧栏+工作区与可收放 inspector 的职责分清；宽屏采用 demo 238侧栏/208会话/弹性正文/254右栏的比例参考，而非强制像素宽。适度窄窗先收右栏为抽屉，再将会话列表变成可返回的选择面；不能让五列的 minWidth 总和把整窗撑出屏幕。原聊天实例、选择与草稿始终驻留，不因切换面板卸载。原窗口装饰用系统真实 traffic lights，不画一套假窗头。

iPhone采用“机组→会话→内容”的原生返回路径，五入口可横向滚动、主动作靠近正文；右栏进入全高sheet。iPad宽屏显示列表+正文，窄分屏沿同一折叠规则，表单不截断、不靠横向拖整个窗。长中文、英文路径/ID、多标签、大字号、软键盘和安全区均纳完成判据。目标验证宽度为 Mac 900/1100/1440、iPhone约390与iPad约768/1024的实际窗口，不以源码width分支冒视觉实测。

## 源文件边界与验证

主要 writer 范围：共享 presentation 原语与 `CrewdHumanActions/CrewdPlanActions`，`MacRootView/CrewSidebarView/CrewCenterView/CrewChatView/CrewSessionWindowView`，`CrewdOriginalWorkIntegration/CrewdWorkPanels/CrewdChannelTodoView/CrewdKnowledgeDocumentsEntry/CrewdCodexControlsView`，`IPadShell/CrewListView`，现连接和quit视图。模型/Core/SQLite/Rust/API和同步存储不作为打磨对象；确有消费缺口一次给根裁定，不用假本地状态顶替。

先统一原语与整体壳，再把整套高频工作流和所有新表单迁入，最后做完整受影响Mac/iOS暖目标与一次候选。期间不为按钮/颜色小修发包，不重复A/C/迁移/配对等稳定业务。冻结交付必须有完整文件范围/准确来源、各尺寸真实截图与交互验收清单：导航、消息与保文、两类待办、知识两阶段、Codex控制、离线/错误/只读、表单键盘、停工与零自启。真实窗口/安装由根协调明确候选执行；开发不干扰当前已装日用App。独立设计评估给出的外部参考与差距意见并入本方案，不建立第二套并写Swift。
