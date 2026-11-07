# 组织表：真实Workspace前置范围读取修复

实际安装的0.2.9在组织表页持续提示“原表格连接已变化，未提交”，刷新不能恢复。后台支持V3表格，但Store首次读取`managedObject.scope`时，真实Workspace代理的精确方法集合不含此必要请求。因此此前直接向Store注入模拟RPC的回归无法覆盖这个断点。

本批在V3 Wire中命名并加入既有的精确`managedObject.scope`，Store使用同一常量。Workspace文件、原连接/能力/Journal/退出检查以及服务端权限没有改变。没有增加通配许可、第二套代理或新的公开服务器方法。Mac和iOS共享该接线；本批尚未新增iOS构建或设备验收。

## 实际验证

- 原缺陷副本只加入真实Workspace测试和原生测试工程接线，保留旧Wire/Store。完整Mac应用和测试编译成功，三项实际执行且全部以正式窗口相同的notSubmitted错误失败。原RED保留。
- 独立修复副本的三项实际执行全部通过、零跳过，覆盖真实Client→Workspace→Store→普通Core的范围/目录/定义检查/建表/加行/当前读取/历史/原UUID只读查回，以及缺Journal、切换机组和Workspace退役后拒绝旧范围。没有注入Workspace RPC或伪造权限。
- RED和GREEN共六个普通Core各使用自己的原休眠UUID，confirmed后自然退出0，socket消失。执行器均禁用，没有模型任务、日用数据或GUI测试。
- 原有完整Wire/Store模拟回归首次真实编译发现runner漏掉新Client依赖的`CrewdAdminAskGroupsWire`。仅补入该真实生产源并把输入数说明改成实际值，重跑build0/run0；原失败保留。这一层仍明确是模拟RPC/持久Journal证据。
- 独立只读复核逐项验证48份证据摘要，未发现新增源码阻断。既有Unknown丢回包、重启和冲突CAS的证据不被这三项测试替代。

完整Root证据位于`/private/tmp/pcw-table-workspace-root-law8q_cm/`。冻结五路径清单`complete-c1/manifest.json`摘要为`8dd1cb987f98346ad855c6e106b82d4c0d88fd347f5af6acc8a83ec5faab4383`。独立报告`/private/tmp/pcw-table-workspace-independent-review-c9b5d1v9/review.md`摘要为`0f0eae45ea6776a4d4ff3c4a53a3c7222693b1848c52fdaa523f38d3168d518f`。

源码与真实Workspace组合关口已完成本地整合。日用安装仍为旧c4的0.2.9，未包含本修复；须冻结新的准确源码、构建签名公证包、正常更新，再在正式窗口验证组织表编辑与恢复，不能据测试通过称窗口故障已关闭。常规同数据格式更新的工具关口也在完善，不通过伪增数据库格式号安装前端修复。无推送或公开发布，全工程目标继续。
