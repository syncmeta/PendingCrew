# PendingCrew 离线公开资料索引

先按问题选页，再在本目录运行 `rg -n '关键词' --glob '*.md' .`；只打开命中段落。

| 问题 | 离线页 | 公开来源 |
| --- | --- | --- |
| 中文快速开始、机组层级、To Do、系统要求、仓库结构 | README-public-v2.md | README.md 全文快照 |
| English quick start, crew, To Do, requirements | README_EN-public-v1.md | README_EN.md full text snapshot |
| 本地 CLI、群聊白板、MCP 工作原理 | architecture-source-v1.md | docs/architecture.md 第 0 节摘录 |
| 此处的演示、问答及双父边 | guide-v1.md | 本机教程示例 |

来源快照：仓库 e427fb2，2026-09-30。README 两页是公开仓库原文的离线副本；图片、相对链接需回仓库查看。architecture 仅收录第 0 节，原文其余章节有带日期的历史分析和实现路径，不作为当前用户操作指南。docs/internal、私有记录与用户数据不入包。维护公开原文时新增版本页和新版索引；安装只补缺失文件，不覆盖用户修改。既有 INDEX.md 保留；以本页为最新入口。