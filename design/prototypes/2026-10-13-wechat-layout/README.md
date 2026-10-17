# PendingCrew 微信式前端原型

本版按用户纠正重做：保留原产品的聊天形态。左 sidebar **只有通讯录、消息两个视图**；通讯录复用原机器分组与可折叠机组层级，消息复用原跨机器扁平、最近活动倒序时间流。右侧选择机组后，**只有消息、Todo、文档、进展四个视图**。消息内有群聊、成员单聊。没有总览、问候、私人团队仪表盘，也没有新增全局会话/方案入口。

打开 `PendingCrew-WeChatLayout.sketch` 后选择“05 消息 · 消息”，按 ⌘P 预览。16 个可编辑画板、185 条原生 Flow 链接。文字/图形均为独立可编辑图层，不是整屏截图。浏览器备选 `index.html` 使用同一图层坐标和链接图；鼠标悬停可显示点击范围。

主要可点击流程：左侧通讯录/消息切换、折叠/展开本机层级、选择 PendingCrew/后端实现机组；右侧四 tab 切换；群聊/单聊；群聊问题 → Todo 完整详情 → 答复；机组文档 → 文档内容；进展 → W12 → 源 Todo。Todo 沿用旧编号、状态圈、新建倒序和待人答复语义。进展引用同一份关联工作数据，不再新造独立状态仪表盘。

原型限制：全部使用合成数据和预填状态；点击答复只跳画板，发送是占位，不实际新增消息。搜索、设置、附件、新建 Todo、未实现的旁支机组为视觉留位。自由文本编辑在 Sketch 画板中进行。文档与进展是界面草图，不证明后台已完成。Jev/记忆未增加 UI。冻结工作显示为只读。

设计来源已只读核对：`Sources/Mac/Views/CrewSidebarView.swift` 固定顶部切换；`CrewDAGTreeView.swift` 机器/父子 DAG；`CrewTimelineListView.swift` 最近活动倒序与来源；`CrewSidebarCrewRow.swift` 同份行样式/谱系色条；`CrewCenterView.swift` 选中机组群聊；`Chat/CrewRosterBar.swift` 成员；`Sources/Support/TodoListPresentation.swift` 编号/状态与答复。旧源码还有总机组视图和三栏成员/终端，原型遵循用户最新指定的两个 sidebar 视图与四个机组视图，没有擅自把旧源码未要求的入口重新加回。

验收：官方 Sketch 2026.3 `sketchtool metadata` 成功解析；`export artboards` 成功导出全部 16 画板。已查看消息时间流和通讯录层级导出图。`validate.py` 检查所用官方 6.5.0 Schema 的 required/type/enum/ref/oneOf/bounds 等约束以及完整性；不是完整通用 JSON Schema 引擎。`native-integrity.json` 记录：185 条 Flow 目标全部存在，图层 ID 全部唯一，只有一个起点。独立无账号 Chrome 实测 **23 个跳转通过，JavaScript 错误为空**；详见 `browser-verification.json`、`browser-verification.log`、两张 browser 截图。

本机 Sketch.app 仍存在此前确认的启动损坏问题（kLSNoExecutableErr），所以没有宣称 Mac app 的 ⌘P 实际播放通过；未安装/修复应用。官方 CLI 解析和图像导出已通过。没有改产品源码，没有提交、push、上传或发布。上一版目录原样保留。

格式依据：Sketch 官方 https://developer.sketch.com/file-format/ 与 https://developer.sketch.com/file-format/spec 。
