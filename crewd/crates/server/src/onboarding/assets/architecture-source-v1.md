# 公开架构导览摘录

Source: docs/architecture.md，`## 0. 三十秒版本`；snapshot: e427fb2（2026-09-30）。只摘当前协作模型；原文含有带日期的历史分析，阅读时应核对其基准。

> 在你自己的机器上把 `claude` / `codex` 这两个 CLI 当**子进程**拉起来，让它们围着一块
> 共享的**群聊白板**协作。白板是磁盘上的一堆 JSON 文件；agent 通过一个 **MCP server**
> 读写它，而那个 MCP server 就是 **app 自己的二进制换个 argv 再跑一遍**。