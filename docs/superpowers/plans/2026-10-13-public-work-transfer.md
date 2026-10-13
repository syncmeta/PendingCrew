# 公开双向工作交接纵向单元

范围来自既定work模型负责人规则：只有现负责人提出，目标本人接受后才改变负责人；人/本工作机长仍可用既有指派。完整工程还需核实失联后的重派、文档/文件夹、真实GUI与交付，本单元不替代这些范围。

## 公共协议与身份

- work.transferRequest：id / expectedRevision / to / note；work.transferAccept及work.transferWithdraw：id / expectedRevision。公共编号和revision保持字符串。
- 实际agent通过MCP work_item_transfer_request/accept/withdraw，可信Sender由既有attach身份与port给出，不能传自称身份。普通本地Human RPC的request/accept按纯权限拒绝，withdraw可人工取消。attached agent连接不能通过普通RPC三新写方法借Human权限。
- 工作及目标机组冻结规则、生存状态、真实session、CAS、跨机组actor作用域沿已有生产边界，不能引入新的无验证身份/副本账。

## 数据与通知不变条件

1. 请求不改变owner；接受才追加ownerHistory（transfer方式、原交接说明、实际接受者）。撤回/拒绝不伪造已交接历史。请求、变更事件及唤醒同一事务，失败全部回滚。
2. 提醒绑定本次真实组织TransferRequested event.seq和当前pending身份；普通进展报告改变revision仍是同一offer，撤回后重发相同目标/正文则是新offer。作者需按既有journal确定实际查询实现，不能仅用当前revision或可重复的毫秒时间当offer身份。
3. 通知与MCP写回执只含有限引用/摘要，完整note由work.get按需读。render与claim都重验实际目标、pending/request身份、冻结、终态和人工停止边界；过期请求不得发送或重启模型。
4. RPC返回完整WorkPlan；MCP摘要是否附目标/请求序号由最终proto明确后固定测试，禁止将长note默认重复回灌。

## 并行所有权与验收

- global_work_resume：当前需求门禁收尾冻结后实现proto/collab/store适配/server+工具目录和生成契约。单一Rust target编译所有者由根协调。
- usage_resume：独占server/tests/work_transfer_workflow.rs，实际socket/MCP的权限/CAS/跨机组/原子故障与actualactor/fake executor投递、过期claim验收。先准备接口失败回归，不改产品。
- usage_independent_review：并行客户端已存在Plan操作，不提前猜交接新接口；保留原设计，只在真实数据与权限边界上接线。
- 根：协议决策、身份绕过边界、源码整合、跨层验收、检查点保存。真实模型/日常数据/安装/main仍按原授权边界，假执行器证据不冒充真实使用。
