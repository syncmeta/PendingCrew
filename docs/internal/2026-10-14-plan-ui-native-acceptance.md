# 计划操作与原生视图离屏验收

源码：计划表单daedc48c，原头像与主题2348dd9e。根验收使用提交快照而非正在编辑的工作目录；每次截图保留source-ref.txt、源码SHA256、renderer程序和PNG，不需应用使用许可、不启动后台或账号。

## 实际执行

- /private/tmp/pcw-plan-forms-backend-01.log，数据 /private/tmp/pcw-work-integration.8XAi9z：实际CrewdPlanCommand构造参数进入正常daemon，公开原始需求生命周期后，从#建计划→active回报/真实检查点→block新H/独立split→答复→typed完成→CAS拒绝与只读重连全部通过。合成源/停止owner、显式fake_codex，无真实模型/额外fake runner流量。
- 前端作者纯表单与草稿回归 /private/tmp/pcw-plan-actions-final.log 及实际SwiftUI类型检查通过；这证明字段/归属/版本边界，不冒充用户实际点击完成。
- 原生像素对照：旧daedc48c显式明暗 /private/tmp/pcw-native-render.WryHX2，dark固定浅背景白字不可读；新2348dd9e /private/tmp/pcw-native-render.IsRjIG，消息泡、tabs/sidebar文字已恢复可读，原哈希头像恢复。根实际查看PNG，未对图像后处理。
- 顶部标题另经同源borderless对照 /private/tmp/pcw-native-render.P8xGoO 验证：文字恢复，低对比来自titled离屏shell的toolbar覆盖。不能据titled截图为产品标题盲目改色；正式可见窗口shell行为仍需单独验收。
- 当前原图假数据不提供完整channel summary/member缓存，成员面板为空不能代表真实群成员展示已验；实际交互/窗口、全部tabs/表单像素、文档和视图文件夹仍未全部验收。

## 后续边界

界面状态将显示中文用户术语，数据层原契约/CAS字符串保留。双向交接API正在后端另单元实施；待目标接受前不会伪造负责人已变。整体工程、main、安装、日常数据迁移、正式发布都未因此完成。
