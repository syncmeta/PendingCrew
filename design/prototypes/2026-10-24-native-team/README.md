# 当前主交付：Sidebar 样式与运行环境收口

打开 `PendingCrew-SwiftUI-Runtime-Environment.sketch`。复用既有 70 态，按客户端静态视觉冻结 `86c4ac2e…` 同步 Sidebar 机组/会话行、footer 与运行环境菜单；原 `PendingCrew-SwiftUI-Sidebar-Navigation.sketch` 保留且 hash 未变。当前说明与验证边界见 [RUNTIME-ENVIRONMENT-DESIGN.md](RUNTIME-ENVIRONMENT-DESIGN.md)，原生 CLI 代表图见 `runtime-previews/`。

本版通过原生格式、70 态可达与 CLI 渲染；没有操作用户 Sketch 窗口，没有本版 GUI 播放，也不代表 Swift App 类型或 runtime 验收。文内以下早期主交付与播放记录均为历史版本。

---

# 当前主交付：SwiftUI 精修 · 侧栏会话导航

打开 `PendingCrew-SwiftUI-Sidebar-Navigation.sketch`。70个原生可编辑状态，sidebar机组根→行尾箭头推入会话子页→返回保留原Tab；右侧铺满，顶层Tab不加分隔线。完整说明、源码对照、Codex四页覆盖与核验边界见 [SIDEBAR-DESIGN.md](SIDEBAR-DESIGN.md)。原生代表图见 `sidebar-previews/`。

当前新文件已通过schema/原生CLI全部70态导出与全图可达检查；因保护用户正在操作的Sketch窗口，未做新侧栏版GUI播放。下文旧49态原型及其GUI记录属于历史版本，不是当前交付验收。所有旧文件与可能的人工设计均保留。

---

# PendingCrew · 私人团队 · 原生 Sketch

主文件：`PendingCrew-PrivateTeam.sketch`。在 Sketch.app 打开，选择「01 群聊」，点击预览/播放按钮。49 个原生画板、6 个可复用 Symbol 源组件、1 个 Symbol 实例。文字和矢量可直接编辑；保存 Sketch 文件即可保留自己的设计修改。

这次沿用相邻 `2026-10-13-folders-chat` 的图层生成器、官方文件格式 schema 和文档打包方式。未改旧文件，未动应用源码、后端、数据库、模型或凭据。

## 建议点击路线

- 群聊 → 新建机组 → 点击任一元数据字段 → 填写机组 → 发送 → 京都机组。
- 人待办 → 确认前端布局 → 更改分组 → 保存分组 → 编辑 → 保存并完成。
- AI 待办 → 详情 → 查看文档 → 设计说明 → 保存片段 → 关联示例 → 生成预览 → 页面预览 → 返回文档。
- 计划进展 → 步骤展开 → 查看设计说明；工作标题独立折叠。
- 群聊 → 成员 → 后端 → 发消息 → 会话列 PendingCrew 返回群聊。消息分类展开与群切换均有独立示例状态。
- 左下机器菜单 → 设置 → 机器与连接 / 通知；选择另一机器查看离线状态。
- 侧栏 PendingCrew 机组（点击模拟右键详情） → Codex 会话 → 模型 / 手动恢复 → 确认恢复 → 叫停工 → 确认 → 退出应用。
- 退出：直接退出 / 安全暂停 / 叫停工并退出。等待窗口可关闭，也可进入强制关闭确认。安全暂停 → 模拟重新打开，明确显示未自动执行。

顶部一条 Tabs 与唯一搜索覆盖会话列和聊天区共同宽度，贴工作区最顶。左 sidebar 全高，底部浅圆 emoji、机器名和向上 chevron 保留。右侧空态只有玻璃加号；中列的右缘中部箭头可折叠并反向展开。

## 验证与边界

Sketch.app 实际打开并播放修正版通过：01 群聊 → 07 新建机组 → 08 填写机组 → 09 已创建机组 → 10 成员 → 点击关闭，以原生 back 返回 09。`native-playback-created.png` 和 `native-playback-back.png` 保存实际播放窗口截图。未改文字图层，未保存至办公空间；文件保持在 Sketch 打开。

当前本机 `sketchtool 2026.3.1 (234013)` 已读入文件并成功导出 49 / 49 画板至 `exports/`。`native-integrity.json` 保存 schema、唯一图层 ID、Symbol 引用、Flow 目标、返回连接和全部画板可达性检查结果。`01 群聊.png` 等是本文件的原生 CLI 渲染，不是产品截图。

这是设计状态原型。发送、编辑、勾选、分组、页面生成、搜索、连接和退出使用预填画板的前后状态；不会执行真实业务。自由输入可在 Sketch 编辑文字图层，原型播放不提供真实文字输入或数据持久化引擎。浮层是保留背景的完整画板状态，使用原生 Flow 和返回连接模拟打开/关闭，不宣称独立 overlay 引擎。右键入口表达为同一详情状态，原型播放使用点击。机组切换展示不同合成内容，运行时数据隔离仍属于产品实现。

`index.html` 随源码保留，仅作同坐标图层的辅助预览；原生返回连接以已实测的 Sketch 播放为准。`draft-evidence/` 保留最初草稿的原文件和导出来源，不纳入当前交付提交。全部动作只使用合成示例，无任何真实联网、模型执行、凭据、发送或发布。

重新生成（会覆盖生成文件，先保留自己的手动设计修改）：

```sh
python3 build.py
python3 validate.py
/Applications/Sketch.app/Contents/MacOS/sketchtool export artboards PendingCrew-PrivateTeam.sketch --output=exports --formats=png --overwriting=YES --serial=YES
```
