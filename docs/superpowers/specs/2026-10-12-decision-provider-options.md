# PendingCrew 判断后端兼容方案

本方案处于讨论阶段。2026-10-12 用户要求先记录兼容选项，商量确定后再实现。现有 TypeSafe Jev 实现继续保留；本次尝试的未提交多后端代码已移为本机私有草稿，不进入提交或推送。

## 目标与边界

把“判断做什么”与“通过哪个服务、模型和账户执行判断”分开。路由、归属、唤醒等业务使用稳定的 Choice、Score、Predicate 请求/结果，Jev 是其中一个实现。确定性规则先处理明确目标、已有负责人、无变化检查等情况，需要判断时才调用后端。

不让整个调度系统依赖 TypeSafe 的专有请求结构，也不为每个后端复制一套业务策略。问题、选项、脱敏、输出校验、期限、成本记录与是否采用结果的策略由公共层管理；协议转换和凭证更新各自隔离。侧栏排序已取消，不因抽象判断层重新引入。

## 兼容选项

| 选项 | 传输与授权 | 结果语义 | 尚需确定或验证 |
|---|---|---|---|
| TypeSafe 官方 Jev | 原生 System One 接口，独立 API key | Choice / Score / Noul 和原生概率/置信度 | 复用已有实现，选定模型版本与测试集 |
| OpenRouter Jev 或其他决策模型 | OpenRouter 原生 Decisions 接口，OpenRouter key | 按实际模型和响应契约映射 | 当前资料入口为 `/api/alpha/decisions`，不能套成 Chat Completions；逐版本核对 |
| OpenAI Decisions | 公共 `/v1/decisions`，API 账户授权 | Predicate / Choice / Score；按名字对应答案，拒绝单独处理 | 模型目录、问题类型和概率语义与 Jev 分开校准；订阅授权能否访问不能从 Responses 权限推断 |
| Responses API 近似判断 | API key，可配置兼容服务 | 用结构化输出生成答案；不是原生分类器概率 | 缺失、拒绝、类型错、截断不能默认成功；模型自报置信度不能冒充校准置信度 |
| ChatGPT 订阅经 Responses | 应用自己的 Sign in with ChatGPT OAuth 授权及允许的账户模型 | 与 Responses 近似判断相同，调用凭证不同 | 官方流程要求注册客户端、专门推理 scopes、`store:false`、`stream:true`；登录、刷新、撤销、超时和完整流终结需实现/验收 |
| 合适的第三方兼容网关 | 按该工具公开授权与服务契约配置 | 根据实际暴露的 Decisions / Responses 能力决定 | 只声明已验证的能力，不读取其他工具的凭证，不把内部 ChatGPT backend-api 当公共接口 |

模型、服务商与账户方式应分别配置。例如同一 Responses 协议可以使用 API key 或应用自己的订阅 OAuth；同一 OpenRouter 服务可提供不同模型。不能把一个 key 在更换服务商后自动沿用，也不能因为两个服务都返回 JSON 就认定接口对等。

来源：[TypeSafe API](https://docs.typesafe.ai/api)、[OpenRouter Jev](https://openrouter.ai/docs/guides/community/jev)、[OpenAI Decisions](https://developers.openai.com/api/docs/guides/decisions)、[订阅推理](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)、[订阅预览限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)。这些是 2026-10-12 文档状态，不代表实际账户或发布版已验证。

## 公共接口方向

请求包含问题类型、稳定问题 ID、候选/级别、必要状态摘要、任务与状态版本、期限。结果包含答案或拒绝、实际模型、后端/授权方式、概率与置信度来源、原始 usage、耗时、重试和错误。源字段缺失明确标为未知，不编造 usage 或概率。

原生概率、LLM 生成估计和离线校准结果分别记录。业务阈值按任务与模型版本校准，不把 Jev 的阈值直接套到其他模型。采用结果是业务配置，不能为省 token 自动放宽权限或新增产品审批。真实 runner 权限仍交人并回传原始决定。

## 降低成本与保持质量

1. 同一有效任务/状态版本可复用判断；负责人、输入、策略、模型或必要上下文变化就使相关缓存失效。时间敏感问题包含相应时间边界。
2. 能合成一批的问题共享状态；确定性筛选先缩小候选，不把每条进度变成对全员的独立询问。
3. 超时不意味着没有产生费用。生成式后端的重试、跨服务 fallback 需明确策略与总期限，不能默认连调所有后端。
4. 诊断记录 provider 原始 token 字段及缓存读写，实际价格/计费规则未知时不显示伪精确费用。账户凭证不进普通配置、日志、提示词或 Git。
5. 用同一有标签的场景集比较质量、误叫醒/漏叫醒、延迟、成本和返工；结构化 JSON 通过不等于判断正确。

## 讨论后再决定的事项

- 首版默认哪个后端，其他选项是手动切换还是自动 fallback？
- Responses 的近似结果何时可自动采用，哪些任务应只给机长建议？
- 每类判断的等待时间、成本目标与失败处理，以及是否允许多后端级联？
- ChatGPT 订阅路线优先直接公共 Responses，还是由已验证工具提供接口？客户端注册和账户切换如何呈现？
- 设置页采用一个全局配置还是按“派活/唤醒/归属”等任务分别配置？

上述未决项不阻塞 Codex 本机首版或上下文降噪工作，也不授权提前开始多后端实现。
