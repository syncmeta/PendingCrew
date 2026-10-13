# 全组织工作账本整合验收 — 2026-10-13

## 本单元结果

一个组织账本承担全部机组的工作状态；公共W/H/#与旧显式别名分开，跨机组parent/dependency/source、负责人权限、阻塞/拆分/人类答复、状态派生与可靠唤醒已贯通。schema9通过真实历史版本链升级，并在升级前保护非空旧库。客户端数据层保持Todo/进展同源、公共身份、结构化错误与只读重连；本单元不包括完整GUI。

生产切线 `4bc8de51`，整合修复 `dca9b21a`，公开契约 `c1addfb0`，独立server/通信夹具 `a273994b`，Swift数据层 `a0684fc4` / 来源关联修复 `25a3f394`，真实组合夹具 `8b43c61e` / 期望修正 `19e14a8c`。最终门禁在同一工作树进行；之后的两处Rust改动仅为测试模块顺序及等价断言写法，已包括在下列最终门禁源码中。

## 已执行证据

- 整套后端：`/private/tmp/pcw-global-workspace-v9-03.log`，83个结果块，2499 passed / 0 failed / 8 ignored。Ignored真实账户等测试未执行，不计入通过。
- 严格静态检查：`/private/tmp/pcw-global-workspace-clippy-02.log`，workspace/all-targets/all-features、warnings denied通过；格式检查与对应失败日志保留。
- 正式Store::open升级、备份拒绝、真实重开：`pcw-global-schema-public-red-01.log` → `pcw-global-schema-public-green-01.log`，2项真实红绿；原始v8快照可读，失败不改数据版本，重开保留新组织状态而不重放旧archive。
- 存储全套：`/private/tmp/pcw-global-store-v9-full-02.log`；包含来源孤儿/别名冲突拒绝、原始来源保留、内部及公共编号高水位、真实事件和事务回滚。
- 协作：`/private/tmp/pcw-global-collab-v9-full-03.log` 与巡检跨机组明细日志；实际H归属和公共H可直接用于明细，越权拒绝保留；新原子默认批处理测试通过。
- 实际socket/MCP：公共block、跨机组owner/split、版本冲突与越权无副作用、精简回执、来源摘要字段均包括在workspace最终门禁。
- 实际actor/fake executor：最终阻塞解除后一次送达；render后完成的提醒在claim被丢弃，无发送或额外启动。
- Swift离线：`/private/tmp/pcw-work-store-swift-final.log`、来源红绿日志保留。
- 真实daemon + Swift组合：`/private/tmp/pcw-work-store-backend-integration-02.log`，数据证据 `/private/tmp/pcw-work-integration.tHQmEy`。合成原始需求经CLI import → 首屏来源关联 → block新H/split → 答复 → CAS currentWork事实 → child/parent完成及需求done派生 → 断线只读恢复。使用显式fake程序，无fake runner流量，更无真实模型唤醒。

真实组合01的失败是夹具误期待H答复后open；从原始DB实测为active/卡点清空，02改正确期望并增加检查点断言，产品未改。全部01/02原始证据、工作目录与失败日志均保留。

## 尚未完成的交付层与工程范围

- 原有微信式侧栏及消息/Todo/文档/进展界面接线、完整窗口验收，未以数据层通过冒充GUI完成。
- 原始需求目前可由真实legacy import进入新账；新的公开手动创建/裁定入口与完整工作交接仍须后续纵向单元验证，不能称三类工作全生命周期都已交付。
- 正式打包/安装、真实账号任务、真实日常数据迁移及回退、main整合和发布分别验收；本轮仅合成数据。
- Claude ACP真实PTY、多厂商完整兼容、后续平台/管理仍在完整工程范围；Codex内部通信延期，Jev与个人/团队记忆先讨论定案。

共享main、用户README与原型/ACP未提交资料未动；不能将本单元完成等同整个大工程完成。
