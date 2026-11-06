# 自由表列类型、生命周期与历史的源码及隔离组合验收

产品源码：`3c144886e2dafd91dcbf10e79b9e7a082cf92929`。从 `9e73c78a` 仅快进合入本地 main，72份产品、协议、测试与工程文件逐摘要一致。五份原有未跟踪设计文件的内容摘要保持原样，未纳入提交。本文为随后补记，不改变所有构建产物的3c来源。

## 完成范围

- Core 格式31→32保护迁移，保留原ID、作者、时间、版本、值位及原回执。11类列、稳定字段与选项身份、安全演进、实际查重确认、表和行各自归档恢复均已接 Human、Agent 与正常共享原生入口。
- T/O有界动作历史、最多4096字节的历史值块和当前权限复核；全值与回包预算保持固定约束。人员和链接字段明确未加载，只有按字段展开才读取真实关联。
- 原6个MCP工具按严格整数版本选择兼容旧V2与新V3，未新增重复工具族；10个新RPC接实际身份及排队后的权限复核。旧224个公开契约展开后相同，当前共234个方法。
- 当前权限与原请求身份分开核验。历史回执校验原意图与原效果，不从最新行或当前结构重构原结果；原UUID不因超时或重开重新提交。

## 实际验收层级

| 层级 | 实际证据 | 边界 |
|---|---|---|
| 源码及回归 | 全工作区实际默认回归终结，原7项失败保留；所有受影响完整目标重跑闭合。后台库952、存储库245、跨组3、远程33、socket23、backup20、混合执行器16、identity15项通过；各目标原有忽略保留，不相加重叠批次。最终全工作区、全targets及testkit严格Clippy和格式检查通过。 | 未把最初失败的整仓运行改写成一次全绿。 |
| 普通Core | 最终653份Rust输入逐Git blob，12生产包实际重新编译，无test/testkit功能。真实旧31→32保护迁移，8类操作／11类列、原UUID、历史分块、正式备份恢复与选项高水位通过；4次Core自然退出。 | 使用隔离合成数据；未改日用库。 |
| 实际MCP stdio | 原6工具、72项无效版本、原始重复键先拒绝且UUID未占用、11类字段／历史／链接、丢回包后新helper查原UUID一次且零重发。最大完整声明31618字节。原Hibernate负责停止同一受控执行器，Core及2helper自然退出，无补救动作。 | 固定假执行器仅提供真实成员身份，不是模型服务验收。 |
| 实际Swift数据层 | 21份最终Git blobs编译并连接同源普通Core；3次Core、4次Swift物理进程正常退出。默认本地远程身份为nil，仍以真实本地目录、数据库和恢复epoch约束完整Scope。Client收到但Store未确认的原AddRow重开后仅lookup一次、零submit；原receipt版本1与当前行版本2分开。人员／工作关联、false／null、精确十进制及实际typed Real位`8000000000000000`通过，原始stdout与位模式交叉核对。 | 原生数据层，不是GUI。Native原休眠线记录较薄，另以未修改冷库中的3个独立runtimeBinding原回执均confirmed且三flags为true补强，不冒称与前两门相同的逐回包记录。 |
| 完整应用 | 同源Mac普通Debug隔离预览及iPhoneOS arm64完整构建通过。Mac内嵌普通后台在签封后机器码等价、严格签名和系统加载闭包通过；iOS核实际BuildStampCommit、iPhone/iPad族与51个保全文件。 | Mac预览未启动／安装；iOS未上真机；不是新公证或公开发布。 |

当前Core真实Unix／TLS和排队后的原令牌／原角色撤销另有专项运行记录。旧schema31及旧Swift客户端的capability回退也有实际进程证据。主线整合后再次逐项核653份Rust与21份Swift冻结输入，并用准确普通后台实际输出API与main文件逐字节比较；没有merge或产品编辑，因此不重复整套相同输入构建。

## 原失败保留

实施阶段修正了类型投影、旧读取拒绝码、原回执校验及相应接线。末轮整仓的7项失败还暴露了旧测试建表绕过新查重、缺真实需求出处、原始SQLite连接未注册函数、工具快照未更新及两个初始化屏障缺口。混合执行器的真实hook stdout证明消息已经注入，修订的是启动回调完成屏障，原唯一提交和20次重连零重发断言保留。

首次普通组合脚本还检出快照BLOB编码、Foundation与POSIX临时路径文本别名、正常本地身份可空、冷WAL模式只读打开及Python解析JSON `-0`丢符号等夹具错误。全部旧脚本和失败现场保留，失败退出／补救信号没有计入通过；最终重新使用全新隔离数据，保持原权限、数据保持、原请求与进程自然退出条件。

## 证明索引

总证明：`/private/tmp/pcw-table-v3-integration.bnxjlm8h/final-integration-proof.json`，SHA256 `a8ec9d8a27f28b0c5a7865f983e24117b4dbbe06f9ca094cac0767169b0dfb93`。

- 普通Core：`/private/tmp/pcw-tables3-ordinary.qvqlvzs7/result.json`，SHA256 `7b008afedbc1ff646b837c5be61dfb97b62930596e8576fc6d29ae2f35a1140b`。
- MCP：`/private/tmp/pcw-tables3-mcp.fzvh1os8/result.json`，SHA256 `8a1003580eada87283d137d0d5f130ca2244c4704024808024c745527425c1f1`。
- Swift/Core：`/private/tmp/pcw-v3-native.bpinmylc/result.json`，SHA256 `a25ae6ac03634f830d33ff8fc73b488d62eb7ed44f5dc37b92ea3d7c20a01013`。
- 最终独立五层复核：`/private/tmp/pcw-tables-v3-quality.h9un7qaj/final-five-layer-combination-review.json`，SHA256 `7808199d0d0bbc9d33b004612321684177163ce0abef8f5d94c32c1cd7c1b157`。

## 保留范围与下一单元

下一完整单元为Type/Object通用查询、共享视图和引用挂载。组织级创建／命名空间、批注／关闭／补偿、导出、完整跨类型历史及相关记忆消费继续原规划；归档不是关闭，历史不是补偿或导出。私有Knowledge/Document目标桥未启用，挂载不得成为授权入口。

正式窗口、真机、日用安装、最新Linux、真实Jev／模型服务与新签名、公证、公开交付仍独立验收。全工程目标持续active，不能由本单元推导全工程完成。

现有Apple公证包仍为0.2.8／20762.18587、源码`a99f433dd2fb8795c980ceca5df95eb563d6db86`。原submission `77390289-1305-44ac-a856-7a31f4a7a099` 已Accepted；本轮仅核对原包摘要及正式签名、票据与Gatekeeper通过，未重复提交，也不把3c新模块标成已公证。
