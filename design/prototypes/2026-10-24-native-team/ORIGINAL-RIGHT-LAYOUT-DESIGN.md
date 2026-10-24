# 原群聊与常驻成员工作区 · 原生可编辑设计

打开 `PendingCrew-SwiftUI-Original-Right-Layout.sketch`。新文件另存，旧 Runtime-Environment / Sidebar-Navigation 保持原字节，没有操作 Sketch GUI 或用户窗口。

以准确 F `431de9dbcec9c41f6d9f11e18d543bf4e7ca076a` 的已验呈现为结构来源，真旧基线为 fe68022f / v0.1.42 的群聊与常驻工作/成员两区。保当前浅色主题、侧栏子页与共同五 Tab / 唯一搜索，消息页恢复群聊常驻，右区为上工作概览、下成员列表，或64pt头像轨与原成员正文/输入。非消息 Tab 仍占整个工作区。

70个原画板ID保留；43个消息基底整体更新，27个非消息/新建/空态的完整画板对象与旧文件相等。旧162个Symbol对象不改，新增9个专用Symbols。原 Window / Sidebar / Workspace Toolbar / Content / Popovers & Sheets 编辑结构保留。新绘部分全为可编辑文字/矢量及Symbol实例，没有铺成截图。原Codex概览/执行/历史/设置与确认/返回浮层保留；成员列表进入原终端，新建入口使用原11号ID改为内联新成员会话，废弃旧详情弹层，未加第71态。

1280×820画布中：侧栏238；群聊641；分隔1；右区400；终端头像轨64+分隔1，正文/输入335。外层左右分隔线不再可拖，与F一致；原上工作/下成员VSplit表达保留。Sketch本身不声称模拟真实拖分栏、双击、自由输入、任务启动或App runtime。设计中的静态已停止样例为合成说明，不补充App缺失的停态样本验收。

复用原schema/ID/Symbol/Flow校验通过：70态从唯一home全部可达、无死目标；2446连线、43个原生back。初次官方CLI全70导出后，自审修正新成员入口并单独导出该态；最终冻结文件再一次批量导出13张代表图到 `original-right-previews/`，未反复全量导出。完整临时图在 `/private/tmp/pendingcrew-original-right-exports-20261025`，不作为提交资产。

文件hash、43/27分界、来源与几何见 `original-right-profile.json`；原生完整性见 `original-right-integrity.json`；旧文件/组件保持及最终代表图来源见 `original-right-verification-boundary.json`。独立质量门尚待接受；CLI静态原型证据不替代App/UI runtime或GUI播放。旧未跟踪/忽略文件仅记录现有字节及本脚本未写入，不冒称它们存在HEAD比对。

Final roster follows actual F source and screenshot: readiness notice and executor disclosure; one inline row for the original work heading, Agent/human ledger pills and original detail button; empty human-work state; original member count and plus-circle new-member row. No invented work headings or interactive member-header entry. Four displayed members are synthetic design examples. All execution/export logs are in the ignored private delivery directory.
