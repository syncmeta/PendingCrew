# PendingCrew Mac 网页交互 demo

本版以现有 PendingCrew SwiftUI 前端为样式来源，补上本设计 session 已确认的交互与工作模型。它是网站可用的本地模拟，不连接真实 crewd，不代表当前安装版或公开发行版已经实现了全部演示功能。

## 打开与网站接入

直接打开 `dist/index.html` 即可运行。HTML、CSS、JavaScript 和全部示例都在 `dist/`，没有 npm 依赖、CDN、外部字体、远程图片或服务端接口。整个目录可以作为静态网站的一个路径发布。

网站版本保留简短介绍、三个示例选择器、使用说明和重置入口。嵌入版本使用 `index.html?embed=1`，去掉外部介绍，窗口占满容器，工具栏保留演示标记。

接入已有网站的例子（路径需要由网站维护者确定）：

```html
<iframe
  src="/demos/pendingcrew/index.html?embed=1"
  title="PendingCrew 交互演示（虚构示例数据）"
  loading="lazy"
  style="width:100%;height:800px;border:1px solid #e2e0d7;border-radius:12px"
></iframe>
```

`PendingCrew-Mac-Web-Demo.zip` 包含可直接放入静态目录的文件。当前只完成本地网页与发布包，未注册新的 Sites 项目、提交/推送或部署现有网站。用户说“到时候放到网站上”，具体网站及接入位置留到网站阶段确定。

## 已对齐的原前端来源

| 来源 | 对齐内容 |
| --- | --- |
| `Sources/Chat/Vendored/Theme.swift` | 原配色、暖黑文字、暖灰材质、深绿按钮、淡绿选中态和用户气泡、18pt 气泡圆角 |
| `Sources/Chat/Vendored/BubbleView.swift` | agent 30pt 头像、发送人、14/10 气泡内边距、0.5pt 细边框，用户右对齐气泡 |
| `Sources/Mac/Views/CrewSidebarView.swift` | 紧凑工具栏、机组选择、机器分组、原层级/最近活动两种视图、隐藏机组与额度/身份页脚 |
| `Sources/Mac/Views/CrewSidebarCrewRow.swift` | 谱系竖色条、状态点、标题/摘要/相对时间、选中行、多父引用 |
| `Sources/Mac/Views/CrewSessionWindowView.swift` | Todo 上、成员下，分别滚动、可拖动上下分隔 |
| PendingBot `Components/GroupAvatar.swift` | 稳定 conversation seed、UInt64 FNV/LCG 选择三枚不同 emoji、白圆灰边、三角布局 |

浏览器使用系统字体与 CSS 近似 Mac 原生控件和侧栏材质。没有启动真实产品窗口作像素对照，也没有声称 CSS 等同 macOS 原生材质。当前设计保持原产品视觉方向，不复用被否决的仪表盘布局。

## 可实际操作的演示

- 三个虚构示例：个人网站改版、周末旅行安排、研究一个主题。也可以在 sidebar 点加号创建新的演示机组。
- 外层 sidebar 只选择机组，保留机器/层级/多父引用与机组活动流；群聊、主题群和单聊的列表只在右侧消息 tab 内。
- 发消息、Enter/Shift+Enter、不同会话的独立草稿、成员单聊、文档/计划/人类事项引用。
- Todo 与成员分别折叠、整栏隐藏，拖动机组/会话分栏与 Todo/成员上下分隔。
- 原始需求、计划、待人类处理三种条目。原始需求保留原文；计划实际展示三层，父项与依赖分开。
- 计划与进展共用同一个 Plan 对象。修改负责人、状态或覆盖式备注，另一视图立即一致。进展不是独立状态副本。
- 同一个全组织待人类收件箱，支持当前机组或全部机组查看。稳定示例编号互不冲突；跨组引用切到源机组。答复解除关联等待，稍后保留等待，不要了结束相关部分，不复制问题。
- 阅读/新建文档、新建原始需求、搜索、独立文件夹的新建/重命名/内容移动。分类改变后稳定 ID 和引用保留。当前文件夹是单层示例；嵌套、拖动移动和引用型分类仍待后续版本。
- “演示下一步”执行一段明确的模拟：完成三层计划中的叶项，并更新它所关联的文档和消息，不调用 AI。
- 浏览器本地保存与每个示例单独重置。localStorage 不可用时交互仍可进行，但刷新后的持久化不能保证。
- 桌面分栏、窄窗口辅助栏、手机会话返回与机组抽屉；`?embed=1` 网站嵌入模式。

## 数据结构与边界

`data.js` 是虚构示例 fixtures，`app.js` 管理操作。原始需求、计划、人类事项、文档、会话和消息分别有稳定 ID；分类关系保存在 placements，四个 scope 独立；计划 parent 表达最多三层的拆解，dependsOn / sources / blockedOn 表达不同关系。全组织人类清单由各机组的同一条事项聚合，答复仍写回来源对象。

来源：已有 spec §24.1 与 `design/prototypes/2026-10-13-original-chat-pane/WORK-MODEL.md`。这是浏览器演示模型，不是新 crewd API 契约。示例没有执行器、调度队列、实际检查点唤醒、权限框或真实交接。按钮反馈、状态变化和成果只作用于演示数据；没有文件系统、部署、预订或任何生产写入。

示例时间是叙事标签，不对应真实任务；“模拟完成”由确定性代码驱动。新增原始需求不会伪造自动生成的真实计划；后续真实产品的 agent 能力需由后端接入。

## 验证与交付

`verify.cjs` 使用独立无账号的 Chrome，对真实 DOM 交互进行检查；记录见 `evidence/browser-verification.json`。涵盖样式数值、群头像稳定性、发送与 HTML 转义、刷新保留、草稿隔离、机组/会话边界、三层折叠、状态不变量、共享备注、全组织收件箱、跨组答复、稍后/撤回、文件夹移动、文档/源需求、模拟回报、搜索、新建机组、重置、可拖动尺寸、手机布局、file URL 与嵌入模式。

截图：`evidence/desktop-chat.png`、`desktop-todo.png`、`desktop-progress.png`、`mobile-chat.png`。只读检视了源码，没有改产品 Swift、Rust、配置、共享计划或根 README，没有切分支、提交、推送、发布或启动/替换产品应用。已有 Sketch 与用户编辑原样保留；这次交付真实网页 demo，不重建 Sketch 画板。
