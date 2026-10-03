# 仓库整理首轮核查（Todo #192，2026-10-03）

基线：`main@cf92594`。本轮只处理有明确废弃证据的已追踪文件；历史记录和本机运行/发行产物分别核对，不按年龄或体积删除。

| 对象 | 证据与处理 |
| --- | --- |
| 根目录 `P4-HANDOFF.md` | 文件第 7、190 行明确要求在合入 main 前删除；其完整实测记录已在 `docs/internal/2026-08-27-p4-handoff-report.md`，该报告所记提交已在当前 main 祖先中。全仓没有其他文件引用根目录交接稿。本轮删除。 |
| `docs/internal/` 的日期记录 | `docs/internal/README.md` 明确规定它们是冻结的施工记录，不是现行说明书；按“过时”批量删除会丢失历史决策与事故证据。本轮保留。 |
| `dist/`、`build/`、`.pendingcrew/` | 都在 `.gitignore` 中，不是已追踪源码；`dist/` 含现用 0.1.43 本机测试包、发布与符号文件，`.pendingcrew/` 含仍在执行的隔离 worktree。本轮不清理运行或回滚材料。 |
| `.wrangler/` 与未追踪的 ACP 调研文件 | 是主工作树原有未追踪内容，归属和迁移状态尚未核实。本轮不触碰。 |

本机容量读数（仅供后续定保留策略）：`dist/` 约 3.1 GB，其中 `symbols/` 约 1.9 GB；`.pendingcrew/worktrees/` 约 7.0 GB；Git 登记 177 个 worktree。清理这些目录前须逐个核实活跃会话、未合入提交、发行回滚和符号归档用途，不能凭体积或旧日期直接 `rm`/`git worktree prune`。

删除交接稿后，`scripts/doc-ref-check.sh` 报 0 条腐烂引用，`git diff --check` 通过。未改产品源码、用户数据、发行包或现场会话。
