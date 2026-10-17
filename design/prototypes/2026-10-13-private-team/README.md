# PendingCrew 前端 Sketch 原型

打开 `PendingCrew-Private-Team.sketch`，在 Sketch 中按 ⌘P 从“01 总览”开始预览。18 个画板、572 个独立可编辑图层、151 条原生 Flow 链接。不是扁平截图；文字与形状可编辑。浏览器备选：打开 `index.html`，按同一链接图跳转。

这是低保真设计草图。所有数据均为合成，所有操作都是画板状态跳转，不连接后台，不消耗模型额度。备注编辑、答复输入是预填的状态示例；Sketch 原型播放不接受自由文本，用户可在画板中修改文字。消息发送是留位，未模拟新增消息。Jev/记忆明确只保留方案。最多 3 层、可读代号与旧别名标记为规划。冻结工作只读。

主要流程：总览 → 机组 → 后端会话 → W12 详情 → 编辑/保存备注；待我处理 → H18 → 答复/稍后/撤回确认；重启提示 → 恢复/稍后；工作 → 冻结工作。左侧导航与浏览器画板选择可直接切页。

验收：Sketch 2026.3 官方 `sketchtool` 成功读取元数据并导出全部 18 张 PNG；已人工查看总览与答复界面的导出。归档检查证明所有原生 Flow 目标存在，图层 ID 无重复，只有 1 个原型起点。生成器按 Sketch 官方 6.5.0 文件格式 schema 检查所用 required/type/enum/ref/oneOf/bounds 等约束；未声称完整通用 JSON Schema 引擎验证。

浏览器实际验收：磁盘空间恢复后，在独立无账号 Chrome 无头窗口执行 `verify.cjs`，13 个流程跳转通过，未出现 JavaScript 错误。包含选机组/成员/计划、备注编辑/保存状态、待人答复/稍后/撤回确认、恢复/稍后和冻结工作只读。`browser-verification.json`、`browser-verification-final.log` 和 `browser-overview.png` 保存结果。自由文本输入不是这个画板原型的功能。

限制：本机 Sketch.app 原生启动失败（“已损坏”，kLSNoExecutableErr）；因此未验证 Mac app 的 ⌘P 播放。早期浏览器失败日志保留，记录沙箱启动失败和磁盘 ENOSPC 阻断；最终浏览器验收已通过。`native-export.log`、`native-metadata.json`、`sketch-probe.log` 保存实际证据。修复应用、安装、上传、发布均未执行。

依据：Sketch 官方文件格式 https://developer.sketch.com/file-format/ 及规范 https://developer.sketch.com/file-format/spec 。
