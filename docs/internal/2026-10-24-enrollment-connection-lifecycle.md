# 配对连接失败与原请求保全

本单元只改变客户端错误说明与人工恢复入口，不改变 TLS 身份校验、配对协议、服务器权限或原请求 UUID。

`CrewdEnrollmentTransport` 在调用 `NWConnection.send` **之前**记录发送交接。一旦调用开始，发送回调错误、超时、取消、断开或无效响应均保持结果未知；发送成功回调也不等于服务器已提交。

只有该次传输明确尚未调用 send，且原 attempt、挑战、凭据、阶段和当前界面操作仍匹配，begin/commit 才恢复原 prepared/challengeReady。不会自动再次调用网络，也不会生成新请求或密钥。旧 Unknown 没有该证据，仍只查询原结果。查询自身失败也不改变原结果；查询代码与本单元前逐字相同。

错误展示仅使用阶段、固定原因和系统错误码，不展示 token、密钥或原始参数。仅真实 `NWPath.unsatisfiedReason.localNetworkDenied` 显示本地网络权限说明；POSIX EACCES/EPERM 仅说明系统拒绝，超时、DNS、TLS 错误不能据此推断权限原因。

关闭、离开、编辑链接或开始另一操作均使旧界面回调失效。凭据操作不会因关闭界面被自动重发或取消；晚到错误不能恢复旧提交入口，晚到摘要不能覆盖新操作。

## 当前证据

- 实际 begin/commit 方法离线切片：原实现 compile 0 / run 1，28 断言中 3 项行为失败；修正后 compile 0 / run 0，33 断言通过。
- 实际 `CrewdRemoteOperation` 离线执行：5 项当前/失效 scope 与晚到回调断言通过。
- 真实 Network socket 代码离线 typecheck 通过；真实 SwiftUI View、Operation 与合成外部依赖 typecheck 通过。四个修改源 parse 通过。
- 无网络、配对、模型、Keychain、运行中 App 或数据操作。未执行完整 Xcode target、iOS target 或新包 GUI；不宣称已修复现场 TLS 无回复或旧配对 Unknown。

证据保存在 `docs/private/preserved-delivery-artifacts/enrollment-connection-lifecycle-20261024/`。可用 `python3 scripts/dev/test-crewd-enrollment-lifecycle.py /private/tmp/新的独立证据目录` 运行上述离线生产切片。已有终态证据目录拒绝覆盖。

源码仅修改主目录；被 InjectionNext 监听的 m6 工作树及原 A/B 进程保持原样。本单元仍需后续隔离的完整构建与正常产品界面验收，不能把离线切片当成候选交付。
