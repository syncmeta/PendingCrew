# M6 正式远程服务源码检查点

日期：2026-10-14。此记录是隔离源码与定向编译层，正式配对/TLS/RPC整链由独立质量线接续，尚非手机可用、真机验收、安装或LAN交付。

## 完整服务边界

- 复用已接受 schema11/TLS primitives。正式 `serve_with` 初始化远程 Manager，默认配置 enabled=0 在创建私钥/绑定网络之前返回；本机 Unix 权限链原样保留。
- 本机 Human 六项配对/管理 API 登记正式方法表与 JSON schema；已附着 agent 不能经普通 RPC 管理配对，远程 Principal 不开放 agent 内部接口/远程管理接口。
- 真 TLS1.3 对端证书经双向 SPKI 校验才生成 Principal；绑定运行时 AppState、独立 authority、dbId、持久 grant、配置代与监听代。每个请求与每个实际发送的事件/答复均核当前授权。hello.remoteBinding 由真实 Principal 填写，不从输入接受身份。
- 配置、邀请、接受与撤销采用真实数据库事务/审计与不可变请求见证。旧 accept UUID 查询返回当前 grant（含撤销墓碑），不重新授权。撤销先提交持久事实，再刷新握手 pins/通知在线连接。
- 已接纳请求的执行不被撤销通知中途取消；旧连接可失去回执，此时属于未知提交，客户端不得自动重放。独立 connection JoinSet 保留执行所有权，监听重配置只更换 intake；全局关闭给已有执行有限 drain。
- 私钥独立文件以0700目录/0600文件保存，拒符号链接/非正规文件/缺失绑定密钥/不同数据库身份，不进入协议或可调试对象。

## 本人执行证据

唯一共享 target `/private/tmp/pcw-decision-target`，Rust1.94.0，CARGO_INCREMENTAL=0、jobs2，默认 features。没有运行服务、LAN listener、App 或模型。

- `pcw-M6-remote-service-check-01.log`：首次编译失败，RpcError不能隐式转anyhow；保存并作有限错误转换。
- `pcw-M6-remote-service-check-02.log`：递归 configure→listener→RPC opaque future Send 判定失败；以明确 boxed listener future消除递归不透明边界。
- `pcw-M6-remote-service-check-03/04/05.log`：实际 cargo check -p crewd-server 通过，05含最终权限处理。
- `pcw-M6-remote-proto-test-01.log`：43通过/1失败，新增无参数 status/grants 的 null 接受登记漏项；独立质量线仅补这两项契约注册。
- `pcw-M6-remote-proto-test-02.log`：44单元测试及2 doctests通过。
- `pcw-M6-remote-schema-export-01.log`：复用本轮实际 proto 编译 rlib 以临时纯输出驱动导出 `api_schema()`，并检查六方法和 Hello.remoteBinding；没有冷编整个 CLI。

## 继续验收

独立质量线的真实 serve_with + 临时 loopback TLS fixture 将验配对签名/challenge/邀请消费、实际工作创建读取、事件订阅、在线撤销与错误权限；另补实际同身份备份恢复后旧UUID确认不复活授权。既有 primitives GREEN 不能替代这些生产路由证明。此阶段不广告远程 capability。后续是原手机/iPad共享工作台的可信配对/Keychain/Native TLS/Root入口及真实设备使用链。
