# crewd M5a Jev 客户端 + 中文实测 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建 crate `crewd-jev`：Jev（TypeSafe System One）的类型化请求 / 应答、带重试与时限的异步客户端、发前凭据脱敏、本机诊断日志、key 存放（macOS 钥匙串 / Linux 0600 文件），以及 spec §7.2 四个决策（入口 Todo 派给谁、人类消息叫醒谁、随手消息归属、侧边栏紧急度）的构造器与判定——输入是普通数据（名单、消息、候选），输出是 `Verdict { best, confidence, outcome: Act(答案) | Defer(原因) }`。再配一套中文实测：40 条合成用例 + `jev-eval` 二进制对真 API 跑，报准确率与按置信度分桶的校准，推荐阈值；最后一个任务真跑一次，把读数写进 spec 新增的「M5a 交付记录与中文实测」一节。

**Architecture:** 三层。**纯计算层**（不碰网络、不读时钟）：`model`（线上类型，choice 选项保序）、`shuffle`（种子打乱）、`prompts`（`prompts/*.md` 编译期载入）、`decisions::{route, wake, attribute, urgency}`（输入 → `Built { state, questions, options }`；应答 → `Verdict`）、`scrub`（正则脱敏）、`eval`（数据集、判对错、汇总）。**IO 层**：`client`（reqwest + rustls，重试 / 超时 / 脱敏的唯一出口）、`key`（钥匙串 / 文件）、`diag`（JSONL）。**门面**：`jev::Jev`——构造 → 脱敏 → 调用 → 判定 → 记日志；未配置不联网直接 `Defer`。`crewd-jev` 不依赖 store / server / exec / proto：接进 crewd 调度是 M5b 的事（需要 M3 的群与成员、M4 的工作项）。

**Tech Stack:** Rust 1.94（edition 2024）、tokio 1.53（rt / macros / time；测试另加 net / io-util）、serde / serde_json、**新增** reqwest 0.13（`default-features = false`，features `rustls`、`system-proxy`）、**新增** regex-lite 0.1、**新增** security-framework 3（仅 macOS，spec §13 Q7 指定）、tempfile（dev）。

**Spec:** `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`（§7 全节、§12、§13 Q7 / Q8、§17「与其它机制的关系」）。Jev 官方文档（2026-10-06 写计划时逐页取回核对，**不凭记忆**）：`https://docs.typesafe.ai/api.md`、`/confidence.md`、`/primitives/choice.md`、`/primitives/noul.md`、`/primitives/score.md`、`/model-jaggedness/jev-1.13.md`。

> 写计划时的验证（照实说哪些跑过、哪些没跑过）：
> - **编译并跑过**（scratch 副本，`--offline`，Cargo.lock 取自 main@aece5fc 的 crewd/）：`model` / `error` / `shuffle` / `prompts` / `decisions/*` / `diag` / `eval`（含数据集自检），47 个单测全绿，`clippy -D warnings` 与 `fmt --check` 干净；`key` 的文件后端（钥匙串模块换成桩）4 个单测全绿；`tests/support` 的假服务器用原始 TCP 自测过（顺序吐应答、记请求、用完回 500）。变异核过会红：去掉打乱（`options_are_the_three_fixed_ones_plus_members_shuffled_by_seed` 红）、wake 的 `>=` 改 `>`（`wake_set_is_everyone_at_or_above_the_line` 红）、`ChoiceCriteria` 按键排序序列化（`choice_criteria_keep_their_order_on_the_wire` 与 `wire_order_matches_options` 都红）。
> - **没编译过**：`scrub.rs`（regex-lite 不在本机缓存）、`client.rs`、`key.rs` 的钥匙串模块、`jev.rs`、`tests/client.rs`、`tests/jev.rs`、`tests/recorded.rs`、`src/bin/jev-eval.rs`（reqwest / security-framework 不在本机缓存）。只做了 `rustfmt` 语法检查；用到的 API 签名在 docs.rs 上逐个核过（reqwest 0.13.5 `ClientBuilder::{connect_timeout, redirect, user_agent, no_proxy}`、`RequestBuilder::{bearer_auth, timeout, body}`、`Error::{is_timeout, without_url}`；security-framework 3.7.0 `passwords::{generic_password(PasswordOptions::new_generic_password(..)), set_generic_password, delete_generic_password}`、`base::Error` 是 `Copy` 且有 `code()` / `message()`）。脱敏的 7 条正则用 Python `re.ASCII`（与 regex-lite 的 ASCII `\b` / `\w` / `\s` 语义一致）把 Task 2 的全部断言跑过一遍，全过。
> - **没做过**：对真 API 的任何调用（Task 9 才做）；Linux 编译。
> - 所以 Task 2 / 3 / 4 / 7 / 8 的「期望通过数」是推算的：对不上先查是不是计划里的代码有错，**别改期望凑数**；改了代码要在提交信息里写明偏离了计划哪一行、为什么。

## Global Constraints

- 所有命令在 `crewd/` 目录下跑；Rust 工具链在 `/opt/homebrew/opt/rustup/bin`（找不到 `cargo` 先 `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`）。
- 依赖：Cargo.lock 里已有的用 `cargo add --offline`；本计划**只**新增三个第三方直接依赖（reqwest、regex-lite、security-framework），用不带 `--offline` 的 `cargo add`，**不手写版本号**。每次加完记下 `grep -c '^name = ' Cargo.lock` 的前后差写进提交信息。`Cargo.lock` 要提交。依赖树里不许出现 openssl / native-tls（Task 3 有检查）。
- 依赖方向：`crewd-jev` 不依赖任何 crewd 内部 crate；也没有任何 crate 依赖它（M5b 才接线）。
- **给 Jev 看的文字只在 `crates/jev/prompts/*.md`**，代码里不写问题、说明、选项描述、档位词的字面量（spec 无此条，是本任务的要求）。提示词正文不含阿拉伯数字（有测试）。
- **不给 Jev 原始日期、时长、计数、排名数字**（spec §7.3 / jaggedness #2 #3）：代码算好换成 `prompts/vocab.md` 里的词。
- **choice 选项每次按种子打乱**（spec §7.3 / jaggedness #8），且**顺序必须原样上线**：请求绝不经 `serde_json::Value` 中转（它的对象按键排序，打乱就白做了）。
- 名单截到最近活跃前 15（spec §7.3）。
- **发前脱敏**（spec §13 Q8）：唯一出口是 `Client::evaluate_raw`，谁调都绕不过；命中次数进诊断日志。
- 诊断日志：本机 JSONL、0600、不记 key、不上传（spec §7.1）。
- key：macOS 钥匙串 service `pendingcrew-jev` account `default`；其他平台配置目录下 `jev-key`（0600）（spec §13 Q7）。key 不进 argv、不打印、不落盘到日志或报告。
- 默认 base URL `https://api.typesafe.ai`、模型 `jev-latest`、低置信度线 0.5（spec §7.3 / §7.4）。base URL 只收 https（本机地址例外，给测试的假服务器）。
- 未配置（关掉或没 key）= 低置信度：不联网，直接 `Defer(Unconfigured)`（spec §7.1）。
- crate 本身不执行任何动作，只返回 `Verdict`：spec §7.3「Jev 结果只驱动可撤回的叫醒 / 归类 / 排序 / 派活」由 M5b 的调用方保证；这里不留任何外部副作用的口子。
- 测试**只用合成数据**（仓库公开，spec §12）。真 API 只在 `jev-eval` 里出现；`check.sh` 永远不跑它，也不跑带 `#[ignore]` 的测试。
- 每个任务结束跑 `./scripts/check.sh`（fmt + clippy -D warnings + test）全绿再提交。提交只 `git add` 该任务列出的文件（路径相对 `crewd/`，提交前看一眼 `git diff --cached --stat` 的文件数对不对），提交信息末尾加 `Co-Authored-By: Claude <noreply@anthropic.com>`（不带模型名）。
- 在分支 `crewd-m5a` 上做（从 main 起，执行时用 worktree；不依赖 M2b）。

## 范围

做：`crates/jev`（包 `crewd-jev`）——线上类型；凭据脱敏；异步客户端（Bearer、单次超时、总时限、429 / 529 / 5xx / 连接错误退避重试、Retry-After、401 / 422 不重试、只收 https）；key 存放；四个决策的构造器与判定（打乱、截名单、量换词、选项校验）；`Outcome::{Act, Defer}`；JSONL 诊断日志（0600、轮转）；门面 `Jev`；中文实测集（40 条）+ `jev-eval`（准确率、校准分桶、阈值推荐、种子间一致率、选中排第一的比例、延迟分位）；真跑一次，录 4 条真实应答原文做夹具，读数写进 spec 新增的「M5a 交付记录与中文实测」一节。

不做（M5b 及以后）：把四个判断接进 crewd 调度（派活绑定、唤醒、写 reply_to、侧边栏排序键合成）；`new_crew` 交机长执行的流程；大厅消息归群后的具体行为（§17「具体行为在 M5 定」——M5a 只让候选能是 `channel`，行为留 M5b）；设置页与「客户端只写 key」的 RPC；行政处的 Jev 判断（§8.3）；把 key 读取接进 `crewd serve`（含钥匙串 ACL 问题，见「待定」）。

## 写计划时定下的裁定（执行时照做，有异议先回报）

1. **crate 叫 `crates/jev`**（包 `crewd-jev`，库 `crewd_jev`）。它对 crewd 其余部分一无所知：输入是普通数据（`Member`、消息文本、`Candidate`），M5b 负责从 store 里拼出这些数据。
2. **HTTP 客户端用 reqwest 0.13**（`default-features = false`，`features = ["rustls", "system-proxy"]`）。Cargo.lock 里没有任何 HTTP 客户端或 TLS 库，必须新增；比较过：ureq 3 是阻塞的（要 `spawn_blocking`），默认 webpki 根证书不认系统钥匙串；直接用 hyper 要自己拼 hyper-util + hyper-rustls + 连接池，代码多。reqwest 的 `rustls` 特性 = rustls（aws-lc-rs）+ rustls-platform-verifier（走系统信任库，公司代理装的根证书也认）；`system-proxy` 让 GUI 拉起的 crewd 跟随 macOS 系统代理（Jev 是唯一外发，连不上就等于全交机长）。不开 `http2`、不开 `json`（自己序列化，才能保住选项顺序）、不开 `charset`。重定向关掉（Authorization 头不跟着跳）；端点是本机地址时 `no_proxy()`。
3. **脱敏用 regex-lite**（零依赖；`\b` / `\w` / `\s` 是 ASCII 语义，正好让中文字符算「非单词」，`是sk-...` 前面有边界）。7 条规则：PEM 私钥块（含没有 END 的半截）、URL 里的 `user:pass@`、`Bearer xxx`、JWT、厂商前缀（`sk-` / `sk_live_` / `rk_live_` / `gh?_` / `github_pat_` / `glpat-` / `xox?-` / `AKIA` / `AIza`）、英文字段赋值（`api_key=` / `password:` …保留字段名）、中文（密码 / 口令 / 密钥 / 私钥 / 令牌 + 是 / 为 / 冒号）。**40 位十六进制（提交号）与 UUID 不遮**：群消息里到处都是。每条规则都排除以 `[` 开头的值，所以脱敏是幂等的（第二遍命中 0）。宁可多遮：误遮一句聊天只损一点准确率。
4. **key 读取**：crewd 运行时走 security-framework（Q7 已定）。**`jev-eval` 走 `/usr/bin/security find-generic-password -w`**（`JEV_API_KEY` 优先）：钥匙串条目若是用 `security add-generic-password` 建的，它的访问控制里信任的是 `security` 这个程序；一个刚编出来、没签名的 `jev-eval` 直接读会弹「允许访问」对话框，agent 点不了 **[推]**。同一个问题 M5b 接进 crewd 时会再遇到（见「待定」）。key 经 stdout 管道传进来，不进任何进程的 argv。
5. **不引入 rand**：SplitMix64 + Fisher–Yates，种子 = FNV-1a(决策名, 正文)。同一输入同一顺序：诊断日志里的 `options` 能复现当时发出的顺序；不同输入的顺序近似均匀（有测试：6 个选项 × 6000 个种子，每个排第一的次数在 800–1200）。
6. **选项名是 ASCII slug**（`[a-z0-9][a-z0-9_-]{0,31}`，唯一，不撞保留名 `captain` / `new_session` / `new_crew` / `none`）；中文名字、职责放在选项描述对象里。官方说选项名也发给模型看，原样回来；用 ASCII 免得中文键在两边的规范化上出岔子 **[推]，中文键没测过**。M5b 用 session 号 / Todo 号拼 slug。
7. **选项保序**：`ChoiceCriteria(Vec<(String, Value)>)` + 手写 `Serialize`（`collect_map` 按插入顺序）；整个请求直接 `serde_json::to_vec`；脱敏在类型上原地做（`scrub_request`），不转 `Value`。
8. **state 是结构化对象**，字段名用英文（`todo`、`message`、`recent`、`crew`…），指令里用反引号指名（官方「Use structure in the questions」的写法）；值是数据或 vocab 词。成员描述 = `{name, role, work(≤3 条), status(忙 / 空闲), activity(刚刚 / 不久前 / 很久)}`。
9. **量换词的分档**：最近活跃排名 1–3 / 4–8 / 其余（含 0 = 不知道）；已等时长 <1h / <8h / <36h / 更久；叫人类次数 0 / 1–2 / ≥3。档位边界有测试。
10. **叫醒**：每个群成员一道 noul（题号 = 成员 key，官方说题号不发给模型），名单上限 30（按最近活跃截）；叫醒集合 = p ≥ `wake_yes`；空集 → `Defer(NobodySelected)`（spec：空集 → 机长）；任何一题缺失 / 类型不对 / 越界 → 整个判断作废（`Malformed`），绝不按「缺的当没叫」处理。整组置信度 = min |2p−1|（官方 confidence 页给 noul 的同尺度量），只记录不设闸（spec 只规定了阈值）。
11. **紧急度**：五级，**不打乱**（score 的级别有序，顺序就是编号）；最可能的级 = probabilities 的 argmax；`confidence < score_min` → `Defer`（调用方退回只按最近活跃排）。
12. **归属**：候选种类有 `message` / `todo` / `work_item` / `channel`（最后一种给 §17 的「大厅消息归哪个群」）；候选最多 20 个（调用方排好序）；外加 `none`。
13. **重试**：最多 3 次（含第一次）；等待 = Retry-After（只认整数秒）否则 300ms × 4^(n−1)，都不超过 2s；重试 429 / 529 / 其余 5xx / 单次超时 / 连接错误；401 / 403 / 其余 4xx / 2xx 但解析不了都不重试。单次 5s、连接 3s、总时限 12s：判断是给调度用的，晚了就没用，宁可交机长 **[推]，数字等 Task 9 的延迟读数校正**。
14. **base URL**：只收 https；http 只许 127.0.0.1 / localhost / [::1]；不许带账号、查询串、片段。不合法在 `Jev::new` 报错（设置页能提示），不悄悄当成未配置。
15. **诊断日志**：每次判断一行（含未配置、输入不合法）；摘要 = 脱敏后 state 的前 160 字符 + 总字符数；选项按发出顺序；文件 0600（已存在的宽权限文件会被收紧）；超过 5 MiB 挪成 `.1`（只留一份）。写失败不影响判断，只计数（`Jev::diag_failures()`）。写是同步小写入（一行几百字节），M5b 若发现阻塞 tokio 线程再挪。
16. **`Verdict::best` 永远保留模型的答案**（即使因低置信度没执行）：评测要用它算准确率；执行只看 `outcome`。
17. **`evaluate_raw` 交出应答体原文**：`jev-eval --record` 存的是原文，不是我们的类型再序列化一遍——夹具要验的正是「真实形状能被我们的类型解析」。
18. **阈值的推荐规则**（`eval`）：choice / score 取 0.30–0.90（步长 0.05）里**最小**的阈值，使置信度 ≥ 它的那部分准确率 ≥ 95% 且至少 3 条；没有满足的 → 推荐「无」（这个决策在中文上不该自动执行）。wake 取 0.20–0.80 里逐成员判对率最高的阈值，并列取离 0.5 最近的。判对错只看模型答案，不看当前阈值。
19. **用例格式**：route / attribute 的 `expect` 是可接受答案集合（任一即对）；wake 分 `expect`（必须叫醒）与 `optional`（叫不叫都对），`optional` 成员不进逐成员统计；urgency 是可接受级别的闭区间。用例故意包含有歧义的几条（`route-06` 接受 new_session 或 captain，`attr-04` 接受 Todo 或对应工作项），校准分桶才有低置信度的样本。

## Review Focus

1. 选项顺序在上线途中被悄悄排序（任何一步经 `serde_json::Value` 中转）→ 打乱形同虚设，jev-1.13 的首选项偏向原样回来，而且没有任何报错。（Task 2 `request_is_scrubbed_in_place_without_reordering_options`、Task 3 `option_order_survives_the_wire`、Task 5 `wire_order_matches_options`）
2. 人在群里贴了 key / 私钥 / 带密码的数据库 URL，这条消息恰好被拿去判叫醒或归属 → 凭据绝不出本机，诊断日志里也没有。（Task 3 `secrets_never_leave_the_machine`、Task 7 `secrets_are_scrubbed_from_the_request_and_the_log`）
3. Jev 挂了 / 很慢 / 一直 429 → 判断不能挂住调度：总时限内返回，交机长。（Task 3 `the_deadline_bounds_the_whole_call`、`gives_up_after_max_attempts`；Task 7 `low_confidence_and_errors_defer_to_the_captain`）
4. 用户关掉 Jev 或没配 key → 一个字节都不外发，全部交机长，日志里写明原因。（Task 7 `without_a_key_nothing_leaves_and_everything_defers`）
5. 模型选了我们没发的选项、漏答某个成员、概率越界 → 绝不执行。（Task 5 `invented_or_missing_answers_are_malformed`、Task 6 `a_missing_member_answer_voids_the_whole_judgement`、`out_of_range_levels_or_scores_are_malformed`）

## 文件结构

```
crewd/crates/jev/
  Cargo.toml                       cargo new + cargo add                                   (T1–T4)
  src/lib.rs                       模块声明（每个任务追加）
  src/model.rs                     Request / Question / ChoiceCriteria（保序）/ Response / Answer   (T1)
  src/scrub.rs                     凭据脱敏：scrub_str / scrub_value / scrub_request                (T2)
  src/error.rs                     CallError（按状态码分类、可否重试）                             (T3)
  src/client.rs                    ApiKey、RetryPolicy、ClientConfig、Client::evaluate(_raw)        (T3)
  src/key.rs                       KeyStore（钥匙串 / 0600 文件）                                  (T4)
  src/shuffle.rs                   seed_from（FNV-1a）、shuffle（SplitMix64 + Fisher–Yates）        (T5)
  src/prompts.rs                   prompts/*.md 的解析与校验                                      (T5)
  src/decisions/mod.rs             Member、Thresholds、Built、Verdict、Outcome、DeferReason、量换词   (T5，T6 追加模块)
  src/decisions/route.rs           入口 Todo 派给谁                                               (T5)
  src/decisions/wake.rs            人类消息叫醒谁                                                 (T6)
  src/decisions/attribute.rs       随手消息归属                                                   (T6)
  src/decisions/urgency.rs         侧边栏紧急度                                                   (T6)
  src/diag.rs                      JSONL 诊断日志                                                 (T7)
  src/jev.rs                       门面 Jev、JevConfig                                            (T7)
  src/eval.rs                      数据集、判对错、汇总、Markdown                                  (T8)
  src/bin/jev-eval.rs              对真 API 跑实测集                                              (T8)
  prompts/{route_todo,wake,attribute,urgency,vocab}.md    给 Jev 看的全部文字                      (T5)
  eval/cases.zh.json               40 条合成中文用例                                              (T8)
  eval/results/<日期>.json          Task 9 的实测报告                                              (T9)
  tests/fixtures/docs-shapes.json  按官方文档形状手写的应答                                        (T1)
  tests/fixtures/recorded/*.json   Task 9 录下的 4 条真实应答原文                                   (T9)
  tests/support/mod.rs             假 Jev（本机 HTTP/1.1）                                         (T3)
  tests/client.rs                  客户端：形状、重试、超时、脱敏、保序                               (T3)
  tests/jev.rs                     门面：未配置、端到端、诊断日志                                   (T7)
  tests/recorded.rs                真实应答原文能被解析                                            (T9)
crewd/Cargo.lock                                                                          (T1–T4)
crewd/README.md                    两行：crates/jev、Jev 评测                                     (T8)
docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md  §3.3 一行、§7.3 一行、新增「M5a 交付记录与中文实测」一节、可能 §13 一条（T9）
```

## 对外接口一览（M5b 只用这些）

```rust
// 配置与 key
pub struct JevConfig { pub enabled: bool, pub base_url: String, pub model: String, pub thresholds: Thresholds }
pub enum KeyStore { Keychain { service: String, account: String }, File(PathBuf) }
impl KeyStore { fn platform_default(config_dir: &Path) -> KeyStore; fn load(&self) -> Result<Option<ApiKey>, KeyError>;
                fn store(&self, key: &ApiKey) -> Result<(), KeyError>; fn clear(&self) -> Result<(), KeyError> }
pub struct DiagLog;  impl DiagLog { fn open(path) -> io::Result<DiagLog> }

// 门面
impl Jev {
    pub fn new(config: &JevConfig, key: Option<ApiKey>, diag: Option<DiagLog>) -> Result<Jev, String>;
    pub fn is_configured(&self) -> bool;
    pub async fn route_todo(&self, input: &RouteTodoInput) -> Verdict<RouteTarget>;   // Captain | Member(key) | NewSession | NewCrew
    pub async fn wake(&self, input: &WakeInput) -> Verdict<WakeSet>;                  // { wake: Vec<key> }
    pub async fn attribute(&self, input: &AttributeInput) -> Verdict<Attribution>;    // Candidate(key) | None
    pub async fn urgency(&self, input: &UrgencyInput) -> Verdict<Urgency>;            // { score: 0.0–4.0, level: 0–4 }
}
pub struct Verdict<T> { pub best: Option<T>, pub confidence: Option<f64>, pub outcome: Outcome<T> }
pub enum Outcome<T> { Act(T), Defer(DeferReason) }
pub enum DeferReason { Unconfigured, InvalidInput(String), Call(CallError), Malformed(String), LowConfidence { confidence: f64 }, NobodySelected }
```

---

### Task 1: crate 骨架 + 线上类型（`model`）

请求 / 应答的形状全部照官方 API 文档（2026-10-06 取回核对）：`POST {base}/v1/systemone`，body `{model, state, questions: {题号: {type, instructions, criteria}}}`；应答 `{model, answers: {题号: Answer}, usage}`。choice 的 `criteria` 是「选项名 → 描述（可为 null）」，score 的是有序数组（2–10 级），noul 的是可选 `{true, false}`。choice / score 的答案带 `confidence`，noul 不带（官方：p 本身就是确定度；需要同尺度时用 |2p−1|）。

**Files:**
- Create: `crewd/crates/jev/Cargo.toml`（`cargo new` + `cargo add`）、`crewd/crates/jev/src/lib.rs`、`crewd/crates/jev/src/model.rs`、`crewd/crates/jev/tests/fixtures/docs-shapes.json`
- Modify: `crewd/Cargo.lock`

**Interfaces:**
- Consumes: 无。
- Produces（`crewd_jev::model`）：
  - 常量 `DEFAULT_BASE_URL: &str`（`"https://api.typesafe.ai"`）、`DEFAULT_MODEL: &str`（`"jev-latest"`）
  - `struct Request { model: String, state: Value, questions: BTreeMap<String, Question> }`（`Serialize`）
  - `enum Question { Choice { instructions: Value, criteria: ChoiceCriteria }, Score { instructions: Value, criteria: Vec<Value> }, Noul { instructions: Value, criteria: Option<NoulCriteria> } }`
  - `struct ChoiceCriteria(pub Vec<(String, Value)>)`：`fn keys(&self) -> Vec<String>`；序列化成 JSON 对象且保持插入顺序
  - `struct NoulCriteria { yes: Value, no: Value }`（线上是 `true` / `false`）
  - `struct Response { model: String, answers: BTreeMap<String, Answer>, usage: Option<Usage> }`（`Serialize + Deserialize`）
  - `enum Answer { Choice { choice: String, probabilities: BTreeMap<String, f64>, confidence: f64 }, Score { score: f64, legend: BTreeMap<String, Value>, probabilities: BTreeMap<String, f64>, confidence: f64 }, Noul { noul: f64 } }`
  - `struct Usage { input_tokens: u64, output_tokens: u64 }`、`fn noul_confidence(p: f64) -> f64`

- [ ] **Step 1: 建 crate、加依赖**

```bash
cargo new --lib crates/jev --name crewd-jev --vcs none
cargo add --offline -p crewd-jev serde --features derive
cargo add --offline -p crewd-jev serde_json
cargo add --offline -p crewd-jev tokio --features rt,macros,time
cat crates/jev/Cargo.toml
```

Expected（workspace 继承的字段 cargo 会自动写；若不是，手工改成这样）：

```toml
[package]
name = "crewd-jev"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
tokio = { version = "1.53.2", features = ["rt", "macros", "time"] }
```

- [ ] **Step 2: 写失败的测试**

`crates/jev/src/lib.rs`（整个文件，覆盖 `cargo new` 生成的内容）：

```rust
//! Jev（TypeSafe System One）客户端：机长的一部分，替机长做高频小判断（spec §7）。
//! 置信度够 → `Outcome::Act` 直接执行；不够 / 没配置 / 出错 → `Outcome::Defer` 交机长。
pub mod model;
```

`crates/jev/tests/fixtures/docs-shapes.json`（按官方文档的应答形状手写，三种答案各一）：

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "route": {
      "type": "choice",
      "choice": "scroll-fix",
      "probabilities": { "scroll-fix": 0.8, "captain": 0.15, "new_session": 0.05 },
      "confidence": 0.7
    },
    "wake_release": {
      "type": "noul",
      "noul": 0.08
    },
    "urgency": {
      "type": "score",
      "score": 1.9,
      "legend": { "0": "不急", "1": "有空看", "2": "在等人" },
      "probabilities": { "0": 0.0, "1": 0.1, "2": 0.9 },
      "confidence": 0.85
    }
  },
  "usage": { "input_tokens": 412, "output_tokens": 57 }
}
```

`crates/jev/src/model.rs`：先只放测试模块（下一步把实现写在它**上面**）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn choice_criteria_keep_their_order_on_the_wire() {
        let mut questions = BTreeMap::new();
        questions.insert(
            "route".to_string(),
            Question::Choice {
                instructions: json!("谁来做？"),
                criteria: ChoiceCriteria(vec![
                    ("zeta".into(), json!("最后一个字母开头，但排在第一")),
                    ("alpha".into(), Value::Null),
                    ("mid".into(), json!({"role": "x"})),
                ]),
            },
        );
        let req = Request {
            model: DEFAULT_MODEL.into(),
            state: json!({"todo": "修滚动"}),
            questions,
        };
        let text = serde_json::to_string(&req).unwrap();
        assert_eq!(
            text,
            r#"{"model":"jev-latest","state":{"todo":"修滚动"},"questions":{"route":{"type":"choice","instructions":"谁来做？","criteria":{"zeta":"最后一个字母开头，但排在第一","alpha":null,"mid":{"role":"x"}}}}}"#
        );
    }

    #[test]
    fn noul_and_score_questions_serialize_like_the_docs() {
        let noul = Question::Noul {
            instructions: json!("要叫醒吗？"),
            criteria: Some(NoulCriteria {
                yes: json!("是"),
                no: json!("否"),
            }),
        };
        assert_eq!(
            serde_json::to_value(&noul).unwrap(),
            json!({"type": "noul", "instructions": "要叫醒吗？", "criteria": {"true": "是", "false": "否"}})
        );
        let bare = Question::Noul {
            instructions: json!("q"),
            criteria: None,
        };
        assert_eq!(
            serde_json::to_value(&bare).unwrap(),
            json!({"type": "noul", "instructions": "q"})
        );
        let score = Question::Score {
            instructions: json!("多急？"),
            criteria: vec![json!("不急"), json!("急")],
        };
        assert_eq!(
            serde_json::to_value(&score).unwrap(),
            json!({"type": "score", "instructions": "多急？", "criteria": ["不急", "急"]})
        );
    }

    #[test]
    fn documented_answer_shapes_parse() {
        let text = include_str!("../tests/fixtures/docs-shapes.json");
        let resp: Response = serde_json::from_str(text).unwrap();
        assert_eq!(resp.model, "jev-1.13.0");
        assert_eq!(
            resp.usage,
            Some(Usage {
                input_tokens: 412,
                output_tokens: 57
            })
        );
        match &resp.answers["route"] {
            Answer::Choice {
                choice,
                probabilities,
                confidence,
            } => {
                assert_eq!(choice, "scroll-fix");
                assert_eq!(probabilities.len(), 3);
                assert!((confidence - 0.7).abs() < 1e-9);
            }
            other => panic!("route 应是 choice：{other:?}"),
        }
        match &resp.answers["wake_release"] {
            Answer::Noul { noul } => assert!((noul - 0.08).abs() < 1e-9),
            other => panic!("wake_release 应是 noul：{other:?}"),
        }
        match &resp.answers["urgency"] {
            Answer::Score {
                score,
                legend,
                probabilities,
                confidence,
            } => {
                assert!((score - 1.9).abs() < 1e-9);
                assert_eq!(legend.len(), 3);
                assert_eq!(probabilities["2"], 0.9);
                assert!((confidence - 0.85).abs() < 1e-9);
            }
            other => panic!("urgency 应是 score：{other:?}"),
        }
    }

    #[test]
    fn unknown_answer_type_is_an_error_not_a_default() {
        let text = r#"{"model":"m","answers":{"x":{"type":"ranking","order":[]}}}"#;
        assert!(serde_json::from_str::<Response>(text).is_err());
    }

    #[test]
    fn noul_confidence_is_distance_from_a_coin_flip() {
        assert_eq!(noul_confidence(0.5), 0.0);
        assert_eq!(noul_confidence(1.0), 1.0);
        assert_eq!(noul_confidence(0.0), 1.0);
        assert!((noul_confidence(0.8) - 0.6).abs() < 1e-9);
    }
}
```

- [ ] **Step 3: 跑一遍确认失败**

Run: `cargo test -p crewd-jev --lib model`
Expected: 编译失败，`cannot find type `Request` in this scope` 之类（实现还没写）。

- [ ] **Step 4: 写实现**（放在 `model.rs` 测试模块的上面）

```rust
//! `POST {base}/v1/systemone` 的请求 / 应答类型（官方文档 docs.typesafe.ai/api.md，2026-10-06 核过）。
//! 选项顺序就是发给模型的顺序（打乱选项防偏向第一个），所以 choice 的 criteria 用有序的 Vec，
//! 序列化成 JSON 对象时保持插入顺序。
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// 官方默认 base URL 与模型名（spec §7.4）。
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
pub const DEFAULT_MODEL: &str = "jev-latest";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Request {
    pub model: String,
    pub state: Value,
    pub questions: BTreeMap<String, Question>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Choice {
        instructions: Value,
        criteria: ChoiceCriteria,
    },
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
    Noul {
        instructions: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
}

/// 有序的「选项名 → 描述」。描述为 `Value::Null` 表示不需要额外说明（官方允许 null）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChoiceCriteria(pub Vec<(String, Value)>);

impl ChoiceCriteria {
    pub fn keys(&self) -> Vec<String> {
        self.0.iter().map(|(k, _)| k.clone()).collect()
    }
}

impl Serialize for ChoiceCriteria {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub yes: Value,
    #[serde(rename = "false")]
    pub no: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        #[serde(default)]
        legend: BTreeMap<String, Value>,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Noul {
        noul: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// 官方给 noul 不返回 confidence；文档建议的同尺度量是 |2p − 1|（docs.typesafe.ai/confidence.md）。
pub fn noul_confidence(p: f64) -> f64 {
    (2.0 * p - 1.0).abs().clamp(0.0, 1.0)
}
```

- [ ] **Step 5: 跑通**

Run: `cargo test -p crewd-jev --lib model`
Expected: `test result: ok. 5 passed`。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/jev/Cargo.toml crates/jev/src/lib.rs crates/jev/src/model.rs crates/jev/tests/fixtures/docs-shapes.json Cargo.lock
git diff --cached --stat
git commit -m "crewd-jev: crate skeleton and System One wire types (order-preserving choice criteria)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: 凭据脱敏（`scrub`）

spec §13 Q8：发前做凭据正则脱敏，命中计数进诊断日志。规则与取舍见裁定 3。`scrub_request` 原地改请求里的每个字符串值（state、每道题的说明与选项描述），**不经 `Value` 中转整个请求**（裁定 7）。

**Files:**
- Create: `crewd/crates/jev/src/scrub.rs`
- Modify: `crewd/crates/jev/src/lib.rs`、`crewd/crates/jev/Cargo.toml`、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: `model::{Request, Question, ChoiceCriteria}`（Task 1）。
- Produces（`crewd_jev::scrub`）：`const REDACTED: &str`（`"[REDACTED]"`）、`fn scrub_str(text: &str) -> (String, usize)`、`fn scrub_value(value: &mut Value) -> usize`、`fn scrub_request(request: &mut Request) -> usize`（返回值都是命中次数）。

- [ ] **Step 1: 加依赖**

```bash
grep -c '^name = ' Cargo.lock
cargo add -p crewd-jev regex-lite
grep -c '^name = ' Cargo.lock
```

Expected: `crates/jev/Cargo.toml` 多一行 `regex-lite = "0.1.x"`；Cargo.lock 的条目数 +1（regex-lite 零依赖）。对不上就把 `git diff Cargo.lock` 里新增的条目名记进提交信息。

- [ ] **Step 2: 写失败的测试**

`lib.rs` 追加一行（保持字母序）：

```rust
pub mod model;
pub mod scrub;
```

`crates/jev/src/scrub.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChoiceCriteria;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn assert_scrubbed(input: &str, secret: &str, expected_hits: usize) -> String {
        let (out, hits) = scrub_str(input);
        assert!(!out.contains(secret), "没遮住 {secret:?}：{out}");
        assert!(out.contains(REDACTED), "{out}");
        assert_eq!(hits, expected_hits, "{out}");
        out
    }

    #[test]
    fn vendor_keys_and_tokens() {
        let anthropic = "sk-ant-api03-AbCdEf0123456789xyzXYZ_-abcdef";
        assert_scrubbed(&format!("我的 key 是{anthropic}，帮我测一下"), anthropic, 1);
        let openai = "sk-proj-0123456789abcdefghijABCDEFGHIJ";
        let out = assert_scrubbed(&format!("export OPENAI_API_KEY={openai}"), openai, 1);
        assert_eq!(out, "export OPENAI_API_KEY=[REDACTED]");
        let gh = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
        assert_scrubbed(&format!("token 在这 {gh}"), &gh, 1);
        assert_scrubbed("AKIAIOSFODNN7EXAMPLE 是 AWS 的", "AKIAIOSFODNN7EXAMPLE", 1);
        let slack = "xoxb-123456789012-abcdefABCDEF";
        assert_scrubbed(slack, slack, 1);
    }

    #[test]
    fn assignments_keep_the_name_and_hide_the_value() {
        let out = assert_scrubbed("password=hunter2hunter", "hunter2hunter", 1);
        assert_eq!(out, "password=[REDACTED]");
        let out = assert_scrubbed(r#"{"api_key": "abcdef123456"}"#, "abcdef123456", 1);
        assert_eq!(out, r#"{"api_key": "[REDACTED]"}"#);
        let out = assert_scrubbed(
            "Authorization: Bearer abcdefghijklmnop1234",
            "abcdefghijklmnop1234",
            1,
        );
        assert_eq!(out, "Authorization: Bearer [REDACTED]");
        let out = assert_scrubbed("密码：Abc12345!", "Abc12345!", 1);
        assert_eq!(out, "密码：[REDACTED]");
        assert_scrubbed("服务器口令是 qwerty789 记一下", "qwerty789", 1);
    }

    #[test]
    fn urls_private_keys_and_jwts() {
        let out = assert_scrubbed(
            "DATABASE_URL=postgres://admin:s3cretpw@db.example.com:5432/app",
            "s3cretpw",
            1,
        );
        assert_eq!(
            out,
            "DATABASE_URL=postgres://[REDACTED]@db.example.com:5432/app"
        );
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";
        let out = assert_scrubbed(
            &format!("贴一下：\n{pem}\n就这个"),
            "b3BlbnNzaC1rZXktdjEAAAAA",
            1,
        );
        assert_eq!(out, "贴一下：\n[REDACTED]\n就这个");
        assert_scrubbed(
            "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBg（被截断）",
            "MIIEvQIBADANBg",
            1,
        );
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        assert_scrubbed(&format!("token {jwt}"), "dozjgNryP4J3", 1);
    }

    #[test]
    fn ordinary_chat_is_left_alone() {
        for text in [
            "commit 46866a0c9b1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a 修好了",
            "max_tokens=4096, tokens: 1200",
            "密码是多少？",
            "把 token 的过期时间改成两小时",
            "sk-learn 很好用",
            "https://github.com/foo/bar/pull/12",
            "session 3f9c2a7e-1b4d-4e8a-9c3f-2a7e1b4d4e8a 卡住了",
            "password reset 的邮件模板改一下",
        ] {
            let (out, hits) = scrub_str(text);
            assert_eq!((out.as_str(), hits), (text, 0), "误遮了：{text}");
        }
    }

    #[test]
    fn scrubbing_twice_finds_nothing_new() {
        let input = "password=hunter2hunter sk-ant-api03-AbCdEf0123456789xyzXYZ postgres://a:b@h 密码：Abc12345! Bearer abcdefghijklmnop1234";
        let (once, first) = scrub_str(input);
        assert_eq!(first, 5);
        let (twice, second) = scrub_str(&once);
        assert_eq!((twice, second), (once, 0));
    }

    #[test]
    fn request_is_scrubbed_in_place_without_reordering_options() {
        let secret = "sk-ant-api03-AbCdEf0123456789xyzXYZ";
        let mut questions = BTreeMap::new();
        questions.insert(
            "route".to_string(),
            Question::Choice {
                instructions: json!("谁来做"),
                criteria: ChoiceCriteria(vec![
                    ("zeta".into(), json!({"work": [format!("轮换 {secret}")]})),
                    ("alpha".into(), Value::Null),
                ]),
            },
        );
        let mut req = Request {
            model: "jev-latest".into(),
            state: json!({"todo": format!("key 是 {secret}"), "n": 3}),
            questions,
        };
        assert_eq!(scrub_request(&mut req), 2);
        let wire = serde_json::to_string(&req).unwrap();
        assert!(!wire.contains(secret));
        assert!(wire.find("\"zeta\"").unwrap() < wire.find("\"alpha\"").unwrap());
    }
}
```

- [ ] **Step 3: 跑一遍确认失败**

Run: `cargo test -p crewd-jev --lib scrub`
Expected: 编译失败，`cannot find function `scrub_str``。

- [ ] **Step 4: 写实现**（放在测试模块上面）

```rust
//! 发给 Jev 之前的凭据脱敏（spec §13 Q8）：key / token / 私钥 / 口令形状的串换成 `[REDACTED]`，
//! 命中次数进诊断日志。宁可多遮（误遮一句聊天只损一点准确率），不可漏发。
//! 幂等：对已经脱敏过的文本再跑一遍，命中 0 次（每条规则都排除以 `[` 开头的值）。
//! 40 位十六进制（git 提交号）、UUID 不遮——群消息里到处都是，它们不是凭据。
use crate::model::{Question, Request};
use regex_lite::{Captures, Regex};
use serde_json::Value;
use std::sync::LazyLock;

pub const REDACTED: &str = "[REDACTED]";

struct Rule {
    re: Regex,
    /// true：保留第 1 组（字段名、`Bearer `、`scheme://`），只遮后面的值。
    keep_prefix: bool,
    /// 遮完后补在后面的字面量（URL 里的 `@`）。
    suffix: &'static str,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let rule = |pattern: &str, keep_prefix: bool, suffix: &'static str| Rule {
        re: Regex::new(pattern).expect("脱敏正则写错了"),
        keep_prefix,
        suffix,
    };
    vec![
        // PEM 私钥块（没有 END 的半截也遮到文本结尾）。
        rule(
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
            false,
            "",
        ),
        // URL 里的 user:password@
        rule(
            r"([A-Za-z][A-Za-z0-9+.-]*://)[^/\s:@\[]+:[^/\s@]+@",
            true,
            "@",
        ),
        // Authorization: Bearer xxx
        rule(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]{16,}", true, ""),
        // JWT
        rule(
            r"eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
            false,
            "",
        ),
        // 厂商前缀的 key / token。
        rule(
            r"\b(?:sk-[A-Za-z0-9_-]{20,}|sk_live_[A-Za-z0-9]{16,}|rk_live_[A-Za-z0-9]{16,}|gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|glpat-[A-Za-z0-9_-]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35})",
            false,
            "",
        ),
        // key=value / key: value（英文字段名）。
        rule(
            r#"(?i)\b((?:api[_-]?key|apikey|secret[_-]?key|client[_-]?secret|secret|access[_-]?token|auth[_-]?token|refresh[_-]?token|token|password|passwd|pwd|private[_-]?key)["']?\s*[:=]\s*["']?)[^\s"',;\[][^\s"',;]{5,}"#,
            true,
            "",
        ),
        // 中文：密码是 xxx / 口令：xxx
        rule(
            r"((?:密码|口令|密钥|私钥|令牌)\s*(?:是|为|[:：=])\s*)[^\s，。,;；\[]{4,}",
            true,
            "",
        ),
    ]
});

/// 脱敏一段文本，返回（结果，命中次数）。
pub fn scrub_str(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut hits = 0;
    for rule in RULES.iter() {
        let replaced = rule.re.replace_all(&out, |caps: &Captures<'_>| {
            hits += 1;
            let prefix = if rule.keep_prefix {
                caps.get(1).map_or("", |m| m.as_str())
            } else {
                ""
            };
            format!("{prefix}{REDACTED}{}", rule.suffix)
        });
        out = replaced.into_owned();
    }
    (out, hits)
}

/// 原地脱敏一个 JSON 值里的全部字符串（只动值，不动对象的键）。
pub fn scrub_value(value: &mut Value) -> usize {
    match value {
        Value::String(s) => {
            let (clean, hits) = scrub_str(s);
            if hits > 0 {
                *s = clean;
            }
            hits
        }
        Value::Array(items) => items.iter_mut().map(scrub_value).sum(),
        Value::Object(map) => map.values_mut().map(scrub_value).sum(),
        _ => 0,
    }
}

/// 原地脱敏整个请求：state 与每道题的说明 / 选项描述。
/// 不经过 `serde_json::Value` 中转整个请求——那样 choice 选项的顺序会被按键排序（打乱就白做了）。
pub fn scrub_request(request: &mut Request) -> usize {
    let mut hits = scrub_value(&mut request.state);
    for question in request.questions.values_mut() {
        hits += match question {
            Question::Choice {
                instructions,
                criteria,
            } => {
                scrub_value(instructions)
                    + criteria
                        .0
                        .iter_mut()
                        .map(|(_, v)| scrub_value(v))
                        .sum::<usize>()
            }
            Question::Score {
                instructions,
                criteria,
            } => scrub_value(instructions) + criteria.iter_mut().map(scrub_value).sum::<usize>(),
            Question::Noul {
                instructions,
                criteria,
            } => {
                scrub_value(instructions)
                    + criteria
                        .as_mut()
                        .map_or(0, |c| scrub_value(&mut c.yes) + scrub_value(&mut c.no))
            }
        };
    }
    hits
}
```

- [ ] **Step 5: 跑通**

Run: `cargo test -p crewd-jev --lib scrub`
Expected: `6 passed`。某条断言不过时先看是不是 regex-lite 与计划假设的语法不同（`\z`、懒惰量词、中文字符类都在 regex-lite 0.1 文档里列为支持），别为了过测试放宽规则或删负例。

- [ ] **Step 6: 变异核对**（确认测试真能抓到漏遮）

把 `RULES` 里「厂商前缀」那一条整条注释掉，跑 `cargo test -p crewd-jev --lib scrub`：Expected `vendor_keys_and_tokens`、`scrubbing_twice_finds_nothing_new`、`request_is_scrubbed_in_place_without_reordering_options` 红。用编辑器撤销改动（**不要** `git checkout -- <file>`：文件还没提交，会把实现一起丢掉），再跑一遍确认回到 6 passed。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/jev/src/scrub.rs crates/jev/src/lib.rs crates/jev/Cargo.toml Cargo.lock
git diff --cached --stat
git commit -m "crewd-jev: credential scrubber (keys, tokens, private keys, passwords; idempotent, keeps option order)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: 调用失败分类 + 异步客户端（`error`、`client`）+ 假 Jev

重试 / 超时 / base URL 规则见裁定 2、13、14。脱敏在 `evaluate_raw` 里做——这是唯一出口。key 只在 `bearer_auth` 那一处取出。测试用 `tests/support/mod.rs` 的假 Jev：本机 TCP 上的极简 HTTP/1.1，按顺序吐预设应答、记下每个请求（写计划时用原始 TCP 自测过）。

**Files:**
- Create: `crewd/crates/jev/src/error.rs`、`crewd/crates/jev/src/client.rs`、`crewd/crates/jev/tests/support/mod.rs`、`crewd/crates/jev/tests/client.rs`
- Modify: `crewd/crates/jev/src/lib.rs`、`crewd/crates/jev/Cargo.toml`、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: `model::{Request, Response}`（Task 1）、`scrub::scrub_request`（Task 2）。
- Produces:
  - `crewd_jev::error::CallError`：`Unauthorized { status: u16 }`、`Rejected { status: u16, body: String }`、`RateLimited`、`Overloaded`、`Server { status: u16 }`、`Timeout`、`Transport(String)`、`Malformed(String)`；`fn is_retryable(&self) -> bool`、`fn from_status(status: u16, body: &str) -> CallError`；`Display`
  - `crewd_jev::client`：`struct ApiKey`（`fn new(raw: &str) -> Option<ApiKey>`、`fn expose(&self) -> &str`；Debug 打 `ApiKey(***)`）、`struct RetryPolicy { max_attempts: u32, base_delay: Duration, max_delay: Duration }`（`fn delay(&self, failed_attempt: u32, retry_after: Option<Duration>) -> Duration`）、`struct ClientConfig { base_url, api_key, retry, connect_timeout, attempt_timeout, deadline }`（`fn new(base_url: impl Into<String>, api_key: ApiKey) -> ClientConfig`）、`struct CallReport { attempts: u32, latency: Duration, scrub_hits: usize }`、`struct Client`（`fn new(config: ClientConfig) -> Result<Client, String>`、`fn endpoint(&self) -> &str`、`async fn evaluate(&self, request: &Request) -> (Result<Response, CallError>, CallReport)`、`async fn evaluate_raw(&self, request: &Request) -> (Result<(Response, String), CallError>, CallReport)`）、`fn endpoint_for(base_url: &str) -> Result<String, String>`、`fn retry_after(headers: &HeaderMap) -> Option<Duration>`
  - 测试器材 `tests/support`：`FakeJev::start(Vec<Reply>) -> FakeJev`（`.base_url`、`.requests() -> Vec<Recorded>`）、`Reply::{ok, status, header, body, delayed}`、`Recorded::{header, body_text}`、`choice_body(question, choice, confidence) -> String`、`noul_body(&[(&str, f64)]) -> String`

- [ ] **Step 1: 加依赖**

```bash
grep -c '^name = ' Cargo.lock
cargo add -p crewd-jev reqwest --no-default-features --features rustls,system-proxy
cargo add --offline -p crewd-jev --dev tokio --features rt,macros,time,net,io-util
grep -c '^name = ' Cargo.lock
cargo tree -p crewd-jev -e normal | grep -i -E 'openssl|native-tls' ; echo "exit=$?"
cargo tree -p crewd-jev -e features -i reqwest | head -20
```

Expected: `[dependencies]` 多 `reqwest = { version = "0.13.x", default-features = false, features = ["rustls", "system-proxy"] }`，`[dev-dependencies]` 多 tokio（rt / macros / time / net / io-util）；openssl / native-tls 的 grep 没有输出、`exit=1`；features 树里没有 `http2`、`json`、`default-tls`。Cargo.lock 新增条目数（几十条：hyper、rustls、aws-lc-rs、rustls-platform-verifier、url…）记进提交信息。首次编译 aws-lc-sys 要一两分钟，正常。

- [ ] **Step 2: 写失败的测试**

`lib.rs`：

```rust
pub mod client;
pub mod error;
pub mod model;
pub mod scrub;
```

`crates/jev/src/error.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_classified_like_the_docs_say() {
        assert_eq!(
            CallError::from_status(401, ""),
            CallError::Unauthorized { status: 401 }
        );
        assert_eq!(CallError::from_status(429, ""), CallError::RateLimited);
        assert_eq!(CallError::from_status(529, ""), CallError::Overloaded);
        assert_eq!(
            CallError::from_status(503, ""),
            CallError::Server { status: 503 }
        );
        let long = "x".repeat(2000);
        match CallError::from_status(422, &long) {
            CallError::Rejected { status, body } => {
                assert_eq!(status, 422);
                assert_eq!(body.chars().count(), 500);
            }
            other => panic!("{other:?}"),
        }
        assert!(CallError::RateLimited.is_retryable());
        assert!(CallError::Overloaded.is_retryable());
        assert!(!CallError::Unauthorized { status: 401 }.is_retryable());
        assert!(
            !CallError::Rejected {
                status: 422,
                body: String::new()
            }
            .is_retryable()
        );
        assert!(!CallError::Malformed(String::new()).is_retryable());
    }
}
```

`crates/jev/src/client.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_accepts_https_and_local_http_only() {
        assert_eq!(
            endpoint_for("https://api.typesafe.ai").unwrap(),
            "https://api.typesafe.ai/v1/systemone"
        );
        assert_eq!(
            endpoint_for(" https://proxy.example.com/jev/ ").unwrap(),
            "https://proxy.example.com/jev/v1/systemone"
        );
        assert_eq!(
            endpoint_for("http://127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080/v1/systemone"
        );
        assert!(endpoint_for("http://localhost:1").is_ok());
        assert!(endpoint_for("http://[::1]:1").is_ok());
        assert!(
            endpoint_for("http://api.typesafe.ai").is_err(),
            "明文发 key"
        );
        assert!(endpoint_for("ftp://api.typesafe.ai").is_err());
        assert!(endpoint_for("https://u:p@api.typesafe.ai").is_err());
        assert!(endpoint_for("https://api.typesafe.ai/?x=1").is_err());
        assert!(endpoint_for("api.typesafe.ai").is_err());
    }

    #[test]
    fn key_is_trimmed_validated_and_never_printed() {
        let k = ApiKey::new("  abc123\n").unwrap();
        assert_eq!(k.expose(), "abc123");
        assert_eq!(format!("{k:?}"), "ApiKey(***)");
        assert!(ApiKey::new("").is_none());
        assert!(ApiKey::new(" \n").is_none());
        assert!(ApiKey::new("ab c").is_none());
    }

    #[test]
    fn backoff_grows_by_four_and_is_capped() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay(1, None), Duration::from_millis(300));
        assert_eq!(p.delay(2, None), Duration::from_millis(1200));
        assert_eq!(p.delay(3, None), Duration::from_secs(2));
        assert_eq!(p.delay(40, None), Duration::from_secs(2), "不溢出");
        assert_eq!(
            p.delay(1, Some(Duration::from_secs(1))),
            Duration::from_secs(1)
        );
        assert_eq!(
            p.delay(1, Some(Duration::from_secs(60))),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn retry_after_reads_integer_seconds_only() {
        let mut h = HeaderMap::new();
        assert_eq!(retry_after(&h), None);
        h.insert(RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
        h.insert(
            RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after(&h), None);
    }
}
```

`crates/jev/tests/support/mod.rs`（整个文件，测试器材）：

```rust
//! 假 Jev：本机 TCP 上的极简 HTTP/1.1 服务器。按顺序吐出预设的应答，记下收到的每个请求。
//! 每个连接只处理一个请求并 `connection: close`，免得客户端复用连接让计数对不上。
#![allow(dead_code)]
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub delay: Duration,
}

impl Reply {
    pub fn ok(body: impl Into<String>) -> Reply {
        Reply {
            status: 200,
            headers: vec![],
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    pub fn status(status: u16) -> Reply {
        Reply {
            status,
            headers: vec![],
            body: format!("{{\"error\":\"fake {status}\"}}"),
            delay: Duration::ZERO,
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Reply {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Reply {
        self.body = body.into();
        self
    }

    pub fn delayed(mut self, delay: Duration) -> Reply {
        self.delay = delay;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// 头名一律小写。
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub struct FakeJev {
    pub base_url: String,
    recorded: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeJev {
    /// 应答用完以后再来的请求一律 500（测试里多出来的请求会因此显形）。
    pub async fn start(replies: Vec<Reply>) -> FakeJev {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
        let rec = recorded.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let rec = rec.clone();
                let queue = queue.clone();
                tokio::spawn(async move {
                    let Some(req) = read_request(&mut sock).await else {
                        return;
                    };
                    rec.lock().unwrap().push(req);
                    let reply = queue
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or_else(|| Reply::status(500).body("fake: no more replies"));
                    tokio::time::sleep(reply.delay).await;
                    let mut head = format!(
                        "HTTP/1.1 {} Fake\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                        reply.status,
                        reply.body.len()
                    );
                    for (k, v) in &reply.headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(reply.body.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        FakeJev {
            base_url: format!("http://127.0.0.1:{port}"),
            recorded,
        }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.recorded.lock().unwrap().clone()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

async fn read_request(sock: &mut TcpStream) -> Option<Recorded> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos;
        }
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Recorded {
        method,
        path,
        headers,
        body,
    })
}

/// 合法的 choice 应答（选 `choice`，置信度 `confidence`）。
pub fn choice_body(question: &str, choice: &str, confidence: f64) -> String {
    serde_json::json!({
        "model": "jev-1.13.0",
        "answers": { question: {
            "type": "choice",
            "choice": choice,
            "probabilities": { choice: 1.0 },
            "confidence": confidence
        }},
        "usage": { "input_tokens": 300, "output_tokens": 20 }
    })
    .to_string()
}

/// 合法的 noul 应答。
pub fn noul_body(ps: &[(&str, f64)]) -> String {
    let answers: serde_json::Map<String, serde_json::Value> = ps
        .iter()
        .map(|(k, p)| {
            (
                k.to_string(),
                serde_json::json!({"type": "noul", "noul": p}),
            )
        })
        .collect();
    serde_json::json!({ "model": "jev-1.13.0", "answers": answers }).to_string()
}
```

`crates/jev/tests/client.rs`：

```rust
//! 客户端对着假 Jev：请求形状、重试、超时、脱敏、选项顺序。
mod support;

use crewd_jev::client::{ApiKey, Client, ClientConfig, RetryPolicy};
use crewd_jev::error::CallError;
use crewd_jev::model::{ChoiceCriteria, Question, Request};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use support::{FakeJev, Reply, choice_body};

const TEST_KEY: &str = "test-key-not-real-0123456789";

fn config(base: &str) -> ClientConfig {
    let mut c = ClientConfig::new(base, ApiKey::new(TEST_KEY).unwrap());
    c.retry = RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(50),
    };
    c.attempt_timeout = Duration::from_millis(300);
    c.deadline = Duration::from_secs(3);
    c
}

fn request(state: Value) -> Request {
    let mut questions = BTreeMap::new();
    questions.insert(
        "route".to_string(),
        Question::Choice {
            instructions: json!("谁来做"),
            criteria: ChoiceCriteria(vec![
                ("zeta".into(), json!("z")),
                ("alpha".into(), json!("a")),
                ("mid".into(), json!("m")),
            ]),
        },
    );
    Request {
        model: "jev-latest".into(),
        state,
        questions,
    }
}

#[tokio::test]
async fn posts_json_with_bearer_to_v1_systemone() {
    let fake = FakeJev::start(vec![Reply::ok(choice_body("route", "alpha", 0.9))]).await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({"todo": "修滚动"}))).await;
    let response = result.unwrap();
    assert_eq!(response.model, "jev-1.13.0");
    assert_eq!(report.attempts, 1);
    let reqs = fake.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(reqs[0].path, "/v1/systemone");
    assert_eq!(
        reqs[0].header("authorization"),
        Some(&*format!("Bearer {TEST_KEY}"))
    );
    assert_eq!(reqs[0].header("content-type"), Some("application/json"));
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["state"]["todo"], "修滚动");
    assert_eq!(body["questions"]["route"]["type"], "choice");
}

#[tokio::test]
async fn option_order_survives_the_wire() {
    let fake = FakeJev::start(vec![Reply::ok(choice_body("route", "alpha", 0.9))]).await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    client.evaluate(&request(json!({}))).await.0.unwrap();
    let text = fake.requests()[0].body_text();
    let (z, a, m) = (
        text.find("\"zeta\"").unwrap(),
        text.find("\"alpha\"").unwrap(),
        text.find("\"mid\"").unwrap(),
    );
    assert!(z < a && a < m, "选项顺序被重排了：{text}");
}

#[tokio::test]
async fn secrets_never_leave_the_machine() {
    let secret = "sk-ant-api03-AbCdEf0123456789xyzXYZ_-abcdef";
    let fake = FakeJev::start(vec![Reply::ok(choice_body("route", "alpha", 0.9))]).await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (_, report) = client
        .evaluate(&request(
            json!({"todo": format!("用这个 key 测一下 {secret}")}),
        ))
        .await;
    assert_eq!(report.scrub_hits, 1);
    let text = fake.requests()[0].body_text();
    assert!(!text.contains(secret), "{text}");
    assert!(text.contains("[REDACTED]"));
}

#[tokio::test]
async fn rate_limit_and_overload_are_retried_then_succeed() {
    let fake = FakeJev::start(vec![
        Reply::status(429),
        Reply::status(529),
        Reply::ok(choice_body("route", "mid", 0.8)),
    ])
    .await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(report.attempts, 3);
    assert_eq!(fake.requests().len(), 3);
}

#[tokio::test]
async fn gives_up_after_max_attempts() {
    let fake = FakeJev::start(vec![
        Reply::status(429),
        Reply::status(429),
        Reply::status(429),
        Reply::status(429),
    ])
    .await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert_eq!(result.unwrap_err(), CallError::RateLimited);
    assert_eq!(report.attempts, 3);
    assert_eq!(fake.requests().len(), 3, "第四个应答不该被取");
}

#[tokio::test]
async fn retry_after_is_honoured_but_capped() {
    let fake = FakeJev::start(vec![
        Reply::status(429).header("retry-after", "30"),
        Reply::ok(choice_body("route", "zeta", 0.7)),
    ])
    .await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let started = Instant::now();
    let (result, _) = client.evaluate(&request(json!({}))).await;
    assert!(result.is_ok());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "Retry-After 30s 应被封顶到 max_delay"
    );
}

#[tokio::test]
async fn unauthorized_and_rejected_are_not_retried() {
    let fake = FakeJev::start(vec![Reply::status(401)]).await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert_eq!(result.unwrap_err(), CallError::Unauthorized { status: 401 });
    assert_eq!(report.attempts, 1);

    let fake = FakeJev::start(vec![
        Reply::status(422).body("{\"detail\":\"questions.route.criteria\"}"),
    ])
    .await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    match result.unwrap_err() {
        CallError::Rejected { status, body } => {
            assert_eq!(status, 422);
            assert!(body.contains("questions.route.criteria"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(report.attempts, 1);
}

#[tokio::test]
async fn malformed_success_is_not_retried() {
    let fake = FakeJev::start(vec![Reply::ok("not json")]).await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert!(matches!(result, Err(CallError::Malformed(_))), "{result:?}");
    assert_eq!(report.attempts, 1);
}

#[tokio::test]
async fn a_slow_attempt_times_out_and_the_retry_succeeds() {
    let fake = FakeJev::start(vec![
        Reply::ok(choice_body("route", "zeta", 0.9)).delayed(Duration::from_secs(2)),
        Reply::ok(choice_body("route", "zeta", 0.9)),
    ])
    .await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(report.attempts, 2);
}

#[tokio::test]
async fn the_deadline_bounds_the_whole_call() {
    let slow = || Reply::ok(choice_body("route", "zeta", 0.9)).delayed(Duration::from_secs(5));
    let fake = FakeJev::start(vec![slow(), slow(), slow()]).await;
    let mut cfg = config(&fake.base_url);
    cfg.attempt_timeout = Duration::from_secs(10);
    cfg.deadline = Duration::from_millis(400);
    let client = Client::new(cfg).unwrap();
    let started = Instant::now();
    let (result, _) = client.evaluate(&request(json!({}))).await;
    assert_eq!(result.unwrap_err(), CallError::Timeout);
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn nothing_listening_is_a_transport_error_after_retries() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let client = Client::new(config(&format!("http://127.0.0.1:{port}"))).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert!(matches!(result, Err(CallError::Transport(_))), "{result:?}");
    assert_eq!(report.attempts, 3);
}
```

- [ ] **Step 3: 跑一遍确认失败**

Run: `cargo test -p crewd-jev --lib`
Expected: 编译失败（`CallError`、`ApiKey`、`endpoint_for` 等未定义）。

- [ ] **Step 4: 写实现**

`error.rs`（测试模块上面）：

```rust
//! 一次调用失败的分类。错误信息里绝不带 key（key 只在 Authorization 头里，reqwest 的错误不含请求头）。
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// 401 / 403：key 缺失或无效。不重试。
    Unauthorized { status: u16 },
    /// 其余 4xx（422 = 我们的请求体不合官方 schema）。不重试；body 截到 500 字符进诊断日志。
    Rejected { status: u16, body: String },
    /// 429，重试用尽。
    RateLimited,
    /// 529，重试用尽。
    Overloaded,
    /// 其余 5xx，重试用尽。
    Server { status: u16 },
    /// 单次超时或总时限用尽。
    Timeout,
    /// 连接 / TLS / 读应答体失败，重试用尽。
    Transport(String),
    /// 2xx 但应答体解析不了。不重试。
    Malformed(String),
}

impl CallError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            CallError::RateLimited
                | CallError::Overloaded
                | CallError::Server { .. }
                | CallError::Timeout
                | CallError::Transport(_)
        )
    }

    /// 由 HTTP 状态码分类（2xx 不经过这里）。
    pub fn from_status(status: u16, body: &str) -> CallError {
        match status {
            401 | 403 => CallError::Unauthorized { status },
            429 => CallError::RateLimited,
            529 => CallError::Overloaded,
            500..=599 => CallError::Server { status },
            _ => CallError::Rejected {
                status,
                body: body.chars().take(500).collect(),
            },
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Unauthorized { status } => write!(f, "unauthorized ({status})"),
            CallError::Rejected { status, body } => write!(f, "rejected ({status}): {body}"),
            CallError::RateLimited => write!(f, "rate limited (429)"),
            CallError::Overloaded => write!(f, "overloaded (529)"),
            CallError::Server { status } => write!(f, "server error ({status})"),
            CallError::Timeout => write!(f, "timeout"),
            CallError::Transport(m) => write!(f, "transport: {m}"),
            CallError::Malformed(m) => write!(f, "malformed response: {m}"),
        }
    }
}
```

`client.rs`（测试模块上面）：

```rust
//! `POST {base}/v1/systemone` 的异步客户端：Bearer key、发前脱敏、单次超时 + 总时限、
//! 429 / 529 / 5xx / 连接错误按指数退避重试（尊重 Retry-After，有上限），401 / 422 不重试。
use crate::error::CallError;
use crate::model::{Request, Response};
use crate::scrub::scrub_request;
use reqwest::header::{CONTENT_TYPE, HeaderMap, RETRY_AFTER};
use std::fmt;
use std::time::{Duration, Instant};

/// key 只经这里流转：没有 Display，Debug 打星号；`expose` 只在拼 Authorization 头时调用。
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// 去首尾空白；空串或含空白 / 控制字符 → None（钥匙串里多存了换行之类）。
    pub fn new(raw: &str) -> Option<ApiKey> {
        let t = raw.trim();
        if t.is_empty() || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
            None
        } else {
            Some(ApiKey(t.to_string()))
        }
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(***)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// 含第一次。
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(300),
            max_delay: Duration::from_secs(2),
        }
    }
}

impl RetryPolicy {
    /// 第 `failed_attempt` 次（从 1 起）失败后等多久：Retry-After 优先，否则 base × 4^(n−1)；都不超过 max_delay。
    pub fn delay(&self, failed_attempt: u32, retry_after: Option<Duration>) -> Duration {
        let backoff = self
            .base_delay
            .saturating_mul(4u32.saturating_pow(failed_attempt.saturating_sub(1)));
        retry_after.unwrap_or(backoff).min(self.max_delay)
    }
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub base_url: String,
    pub api_key: ApiKey,
    pub retry: RetryPolicy,
    pub connect_timeout: Duration,
    /// 单次请求（连接到读完应答）的上限。
    pub attempt_timeout: Duration,
    /// 含重试与等待的总时限：判断是给调度用的，晚了就没用了，宁可交机长。
    pub deadline: Duration,
}

impl ClientConfig {
    pub fn new(base_url: impl Into<String>, api_key: ApiKey) -> ClientConfig {
        ClientConfig {
            base_url: base_url.into(),
            api_key,
            retry: RetryPolicy::default(),
            connect_timeout: Duration::from_secs(3),
            attempt_timeout: Duration::from_secs(5),
            deadline: Duration::from_secs(12),
        }
    }
}

/// 一次 `evaluate` 的账：试了几次、总耗时、脱敏命中几处。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CallReport {
    pub attempts: u32,
    pub latency: Duration,
    pub scrub_hits: usize,
}

#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    endpoint: String,
    key: ApiKey,
    retry: RetryPolicy,
    attempt_timeout: Duration,
    deadline: Duration,
}

fn is_local(host: Option<&str>) -> bool {
    matches!(host, Some("127.0.0.1") | Some("localhost") | Some("[::1]"))
}

/// base URL → 完整端点。只收 https；http 只许本机（测试用的假服务器）。不许带账号、查询串、片段。
pub fn endpoint_for(base_url: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(base_url.trim()).map_err(|e| format!("base URL 无效：{e}"))?;
    match url.scheme() {
        "https" => {}
        "http" if is_local(url.host_str()) => {}
        other => {
            return Err(format!(
                "base URL 必须是 https（只有本机地址可以用 http），现在是 {other}"
            ));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("base URL 里不能带账号密码".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("base URL 里不能带查询串或片段".into());
    }
    Ok(format!(
        "{}/v1/systemone",
        url.as_str().trim_end_matches('/')
    ))
}

impl Client {
    pub fn new(config: ClientConfig) -> Result<Client, String> {
        let endpoint = endpoint_for(&config.base_url)?;
        let local = reqwest::Url::parse(&endpoint)
            .map(|u| is_local(u.host_str()))
            .unwrap_or(false);
        let mut builder = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            // 不跟随重定向：Authorization 头不跟着跳到别处。
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("crewd-jev/", env!("CARGO_PKG_VERSION")));
        if local {
            // 本机端点不走代理（系统代理 / 环境变量代理都不该看到本机流量，测试也因此稳定）。
            builder = builder.no_proxy();
        }
        let http = builder
            .build()
            .map_err(|e| format!("HTTP 客户端初始化失败：{e}"))?;
        Ok(Client {
            http,
            endpoint,
            key: config.api_key,
            retry: config.retry,
            attempt_timeout: config.attempt_timeout,
            deadline: config.deadline,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// 发一次判断。请求先脱敏（唯一的出口，谁调都绕不过），再按策略重试。
    pub async fn evaluate(&self, request: &Request) -> (Result<Response, CallError>, CallReport) {
        let (result, report) = self.evaluate_raw(request).await;
        (result.map(|(response, _)| response), report)
    }

    /// 同 `evaluate`，另外交出应答体原文（jev-eval 录夹具用）。
    pub async fn evaluate_raw(
        &self,
        request: &Request,
    ) -> (Result<(Response, String), CallError>, CallReport) {
        let started = Instant::now();
        let mut request = request.clone();
        let scrub_hits = scrub_request(&mut request);
        let report = |attempts| CallReport {
            attempts,
            latency: started.elapsed(),
            scrub_hits,
        };
        let body = match serde_json::to_vec(&request) {
            Ok(b) => b,
            Err(e) => {
                return (
                    Err(CallError::Malformed(format!("请求序列化失败：{e}"))),
                    report(0),
                );
            }
        };
        let mut attempts = 0;
        let result = loop {
            let remaining = self.deadline.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break Err(CallError::Timeout);
            }
            attempts += 1;
            let (outcome, retry_after) = self
                .attempt(&body, self.attempt_timeout.min(remaining))
                .await;
            match outcome {
                Ok(response) => break Ok(response),
                Err(e) if e.is_retryable() && attempts < self.retry.max_attempts => {
                    let delay = self.retry.delay(attempts, retry_after);
                    if started.elapsed() + delay >= self.deadline {
                        break Err(e);
                    }
                    tokio::time::sleep(delay).await;
                }
                Err(e) => break Err(e),
            }
        };
        (result, report(attempts))
    }

    async fn attempt(
        &self,
        body: &[u8],
        timeout: Duration,
    ) -> (Result<(Response, String), CallError>, Option<Duration>) {
        let sent = self
            .http
            .post(&self.endpoint)
            .bearer_auth(self.key.expose())
            .header(CONTENT_TYPE, "application/json")
            .timeout(timeout)
            .body(body.to_vec())
            .send()
            .await;
        let response = match sent {
            Ok(r) => r,
            Err(e) => return (Err(transport_error(e)), None),
        };
        let status = response.status().as_u16();
        let wait = retry_after(response.headers());
        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => return (Err(transport_error(e)), wait),
        };
        if (200..300).contains(&status) {
            let parsed = serde_json::from_slice::<Response>(&bytes)
                .map(|r| (r, String::from_utf8_lossy(&bytes).into_owned()))
                .map_err(|e| CallError::Malformed(e.to_string()));
            return (parsed, None);
        }
        (
            Err(CallError::from_status(
                status,
                &String::from_utf8_lossy(&bytes),
            )),
            wait,
        )
    }
}

fn transport_error(e: reqwest::Error) -> CallError {
    if e.is_timeout() {
        CallError::Timeout
    } else {
        CallError::Transport(e.without_url().to_string())
    }
}

/// 只认整数秒的 Retry-After（HTTP 日期形式忽略，走退避）。
pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}
```

- [ ] **Step 5: 跑通**

Run: `cargo test -p crewd-jev --lib`，再 `cargo test -p crewd-jev --test client`
Expected: 单测 `16 passed`（model 5 + scrub 6 + error 1 + client 4）；集成测试 `11 passed`，整组几秒内跑完（最慢的是超时那两条）。

若 `nothing_listening_is_a_transport_error_after_retries` 报的是 `Timeout` 而不是 `Transport`：说明本机把 127.0.0.1 的连接拒绝变成了超时（防火墙 / 代理软件），先 `env | grep -i proxy` 看测试进程有没有继承代理变量——`Client::new` 对本机端点已经 `no_proxy()`，仍然超时就把现象写进提交信息，**不要**改成接受两种结果。

- [ ] **Step 6: 变异核对**

逐条做、每条做完撤销（编辑器撤销，不用 `git checkout`）：
1. `evaluate_raw` 里把 `let scrub_hits = scrub_request(&mut request);` 换成 `let scrub_hits = 0;` → Expected `secrets_never_leave_the_machine` 红。
2. `CallError::is_retryable` 里去掉 `CallError::Overloaded` → Expected `rate_limit_and_overload_are_retried_then_succeed` 与 `statuses_are_classified_like_the_docs_say` 红。
3. `attempt` 里把 `.timeout(timeout)` 删掉 → Expected `a_slow_attempt_times_out_and_the_retry_succeeds` 红（`the_deadline_bounds_the_whole_call` 也可能红）。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/jev/src/error.rs crates/jev/src/client.rs crates/jev/src/lib.rs crates/jev/tests/support/mod.rs crates/jev/tests/client.rs crates/jev/Cargo.toml Cargo.lock
git diff --cached --stat
git commit -m "crewd-jev: async System One client (bearer, scrub-before-send, timeouts, backoff on 429/529/5xx, https only)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: key 存放（`key`）

spec §13 Q7：macOS 钥匙串（security-framework），Linux 配置目录下 0600 文件；key 只由 crewd 读。文件后端：权限比 0600 宽就**拒读**（可能已泄漏，让人处理，不悄悄用）；写入走「同目录临时文件（0600、create_new）→ rename」，任何时刻都没有半截或宽权限的 key 文件。真钥匙串的往返测试默认不跑（`#[ignore]` + 环境变量两道闸），只碰测试专用的 service `pendingcrew-jev-test`。

**Files:**
- Create: `crewd/crates/jev/src/key.rs`
- Modify: `crewd/crates/jev/src/lib.rs`、`crewd/crates/jev/Cargo.toml`、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: `client::ApiKey`（Task 3）。
- Produces（`crewd_jev::key`）：常量 `KEYCHAIN_SERVICE`（`"pendingcrew-jev"`）、`KEYCHAIN_ACCOUNT`（`"default"`）、`KEY_FILE_NAME`（`"jev-key"`）；`enum KeyStore { Keychain { service: String, account: String }, File(PathBuf) }`（`fn platform_default(config_dir: &Path) -> KeyStore`、`fn load(&self) -> Result<Option<ApiKey>, KeyError>`、`fn store(&self, key: &ApiKey) -> Result<(), KeyError>`、`fn clear(&self) -> Result<(), KeyError>`）；`enum KeyError { InsecurePermissions { path, mode }, Invalid, Io(io::Error), Keychain { code: i32, message: String }, Unsupported }`（`Display`）。

- [ ] **Step 1: 加依赖**

```bash
grep -c '^name = ' Cargo.lock
cargo add -p crewd-jev security-framework --target 'cfg(target_os = "macos")'
cargo add --offline -p crewd-jev --dev tempfile
grep -c '^name = ' Cargo.lock
```

Expected: `Cargo.toml` 多一节 `[target.'cfg(target_os = "macos")'.dependencies]` 里 `security-framework = "3.x"`；Cargo.lock 很可能**不变或只多一两条**——reqwest 的 rustls-platform-verifier 在 macOS 上已经带进了 security-framework 3 **[推]**。若 cargo 选了与已有条目不同的大版本，停下回报。

- [ ] **Step 2: 写失败的测试**

`lib.rs`：

```rust
pub mod client;
pub mod error;
pub mod key;
pub mod model;
pub mod scrub;
```

`crates/jev/src/key.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_roundtrips_with_0600_and_clear_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::File(dir.path().join("cfg").join(KEY_FILE_NAME));
        assert!(store.load().unwrap().is_none(), "没存过 = 未配置");
        store.store(&ApiKey::new("test-key-1").unwrap()).unwrap();
        let KeyStore::File(path) = &store else {
            unreachable!()
        };
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(store.load().unwrap().unwrap().expose(), "test-key-1");
        store.store(&ApiKey::new("test-key-2").unwrap()).unwrap();
        assert_eq!(store.load().unwrap().unwrap().expose(), "test-key-2");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("cfg")).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "没有残留的临时文件");
        store.clear().unwrap();
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn loose_permissions_are_refused_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(KEY_FILE_NAME);
        std::fs::write(&path, "test-key").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        match KeyStore::File(path).load() {
            Err(KeyError::InsecurePermissions { mode, .. }) => assert_eq!(mode, 0o644),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn garbage_in_the_file_is_invalid_not_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(KEY_FILE_NAME);
        std::fs::write(&path, "two words").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            KeyStore::File(path).load(),
            Err(KeyError::Invalid)
        ));
    }

    #[test]
    fn platform_default_matches_the_spec() {
        let store = KeyStore::platform_default(Path::new("/cfg"));
        if cfg!(target_os = "macos") {
            assert_eq!(
                store,
                KeyStore::Keychain {
                    service: "pendingcrew-jev".into(),
                    account: "default".into()
                }
            );
        } else {
            assert_eq!(store, KeyStore::File(PathBuf::from("/cfg/jev-key")));
        }
    }

    /// 真钥匙串：写 / 读 / 删一个测试专用条目。默认不跑（check.sh 不带 --ignored）。
    #[test]
    #[ignore = "碰真钥匙串：CREWD_KEYCHAIN_TEST=1 cargo test -p crewd-jev keychain -- --ignored"]
    fn keychain_roundtrip() {
        if std::env::var("CREWD_KEYCHAIN_TEST").as_deref() != Ok("1") || !cfg!(target_os = "macos")
        {
            return;
        }
        let store = KeyStore::Keychain {
            service: "pendingcrew-jev-test".into(),
            account: "crewd-test".into(),
        };
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
        store
            .store(&ApiKey::new("test-key-not-real").unwrap())
            .unwrap();
        assert_eq!(store.load().unwrap().unwrap().expose(), "test-key-not-real");
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }
}
```

- [ ] **Step 3: 跑一遍确认失败**

Run: `cargo test -p crewd-jev --lib key::`
Expected: 编译失败（`KeyStore` 未定义）。

- [ ] **Step 4: 写实现**（测试模块上面）

```rust
//! Jev key 放哪（spec §7.4、§13 Q7）：macOS 钥匙串 service `pendingcrew-jev` account `default`；
//! 其他平台放 crewd 配置目录下的 `jev-key`（0600，权限更宽就拒读）。key 只由 crewd 读，客户端只写。
use crate::client::ApiKey;
use std::fmt;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const KEYCHAIN_SERVICE: &str = "pendingcrew-jev";
pub const KEYCHAIN_ACCOUNT: &str = "default";
pub const KEY_FILE_NAME: &str = "jev-key";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStore {
    Keychain { service: String, account: String },
    File(PathBuf),
}

#[derive(Debug)]
pub enum KeyError {
    /// 文件对组 / 其他人可读写：可能已经泄漏，拒绝使用，让人处理。
    InsecurePermissions {
        path: PathBuf,
        mode: u32,
    },
    /// 存的内容不是合法 key（空、带空白）。
    Invalid,
    Io(io::Error),
    Keychain {
        code: i32,
        message: String,
    },
    /// 这个平台没有钥匙串。
    Unsupported,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::InsecurePermissions { path, mode } => {
                write!(f, "{} 的权限是 {mode:o}，必须是 600", path.display())
            }
            KeyError::Invalid => write!(f, "存的 key 不合法（空或含空白）"),
            KeyError::Io(e) => write!(f, "读写 key 文件失败：{e}"),
            KeyError::Keychain { code, message } => write!(f, "钥匙串错误 {code}：{message}"),
            KeyError::Unsupported => write!(f, "这个平台没有钥匙串"),
        }
    }
}

impl From<io::Error> for KeyError {
    fn from(e: io::Error) -> Self {
        KeyError::Io(e)
    }
}

impl KeyStore {
    /// macOS → 钥匙串；其他 → `<config_dir>/jev-key`。
    pub fn platform_default(config_dir: &Path) -> KeyStore {
        if cfg!(target_os = "macos") {
            KeyStore::Keychain {
                service: KEYCHAIN_SERVICE.into(),
                account: KEYCHAIN_ACCOUNT.into(),
            }
        } else {
            KeyStore::File(config_dir.join(KEY_FILE_NAME))
        }
    }

    /// 没存过 → Ok(None)（= 未配置，判断全交机长）。
    pub fn load(&self) -> Result<Option<ApiKey>, KeyError> {
        let raw = match self {
            KeyStore::Keychain { service, account } => keychain::load(service, account)?,
            KeyStore::File(path) => file_load(path)?,
        };
        match raw {
            None => Ok(None),
            Some(bytes) => {
                let text = String::from_utf8(bytes).map_err(|_| KeyError::Invalid)?;
                ApiKey::new(&text).map(Some).ok_or(KeyError::Invalid)
            }
        }
    }

    pub fn store(&self, key: &ApiKey) -> Result<(), KeyError> {
        match self {
            KeyStore::Keychain { service, account } => {
                keychain::store(service, account, key.expose().as_bytes())
            }
            KeyStore::File(path) => file_store(path, key.expose()),
        }
    }

    /// 删掉；本来就没有也算成功。
    pub fn clear(&self) -> Result<(), KeyError> {
        match self {
            KeyStore::Keychain { service, account } => keychain::clear(service, account),
            KeyStore::File(path) => match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
        }
    }
}

fn file_load(path: &Path) -> Result<Option<Vec<u8>>, KeyError> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(KeyError::InsecurePermissions {
            path: path.to_path_buf(),
            mode,
        });
    }
    Ok(Some(std::fs::read(path)?))
}

/// 写到同目录的临时文件（0600、create_new）再改名：任何时刻都没有一个半截或权限宽的 key 文件。
fn file_store(path: &Path, key: &str) -> Result<(), KeyError> {
    let dir = path.parent().ok_or(KeyError::Invalid)?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let result = write_private(&tmp, key).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(KeyError::from)
}

fn write_private(path: &Path, key: &str) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(key.as_bytes())?;
    f.sync_all()
}

#[cfg(target_os = "macos")]
mod keychain {
    use super::KeyError;
    use security_framework::passwords::{
        PasswordOptions, delete_generic_password, generic_password, set_generic_password,
    };

    /// errSecItemNotFound
    const ITEM_NOT_FOUND: i32 = -25300;

    fn err(e: security_framework::base::Error) -> KeyError {
        KeyError::Keychain {
            code: e.code(),
            message: e.message().unwrap_or_default(),
        }
    }

    pub fn load(service: &str, account: &str) -> Result<Option<Vec<u8>>, KeyError> {
        match generic_password(PasswordOptions::new_generic_password(service, account)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(err(e)),
        }
    }

    pub fn store(service: &str, account: &str, secret: &[u8]) -> Result<(), KeyError> {
        set_generic_password(service, account, secret).map_err(err)
    }

    pub fn clear(service: &str, account: &str) -> Result<(), KeyError> {
        match delete_generic_password(service, account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(err(e)),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod keychain {
    use super::KeyError;

    pub fn load(_: &str, _: &str) -> Result<Option<Vec<u8>>, KeyError> {
        Err(KeyError::Unsupported)
    }

    pub fn store(_: &str, _: &str, _: &[u8]) -> Result<(), KeyError> {
        Err(KeyError::Unsupported)
    }

    pub fn clear(_: &str, _: &str) -> Result<(), KeyError> {
        Err(KeyError::Unsupported)
    }
}
```

- [ ] **Step 5: 跑通**

Run: `cargo test -p crewd-jev --lib key::`
Expected: `4 passed; 1 ignored`。单测总数 20（另 1 ignored）。

- [ ] **Step 6（可选，人在机器前时做）: 真钥匙串往返**

Run: `CREWD_KEYCHAIN_TEST=1 cargo test -p crewd-jev --lib keychain_roundtrip -- --ignored`
Expected: `1 passed`。钥匙串锁着时系统会弹解锁框——agent 点不了，跳过这步并在提交信息里写「钥匙串往返没跑」。它只建、读、删 `pendingcrew-jev-test` / `crewd-test` 这一条，不碰真 key。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/jev/src/key.rs crates/jev/src/lib.rs crates/jev/Cargo.toml Cargo.lock
git diff --cached --stat
git commit -m "crewd-jev: key store (macOS keychain pendingcrew-jev/default, 0600 file elsewhere; loose perms refused)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: 提示词文件 + 打乱 + 决策公共部分 + 派活（`prompts`、`shuffle`、`decisions`、`decisions::route`）

全部给 Jev 看的文字在 `prompts/*.md`：格式是「第一个 `## 名字` 之前是给人看的注释，之后每节到下一个 `## ` 为止」。五个文件这一任务一次建齐（`prompts.rs` 的测试要求每个文件的节都在）。决策公共部分：成员、阈值、`Built`、`Verdict` / `Outcome` / `DeferReason`、量换词（裁定 9）、选项名校验（裁定 6）、读 choice 答案并核对、按阈值闸门。派活：一道 choice，选项 = captain / new_session / new_crew / 每个活跃成员（截前 15），按种子打乱。

**Files:**
- Create: `crewd/crates/jev/prompts/{route_todo,wake,attribute,urgency,vocab}.md`、`crewd/crates/jev/src/prompts.rs`、`crewd/crates/jev/src/shuffle.rs`、`crewd/crates/jev/src/decisions/mod.rs`、`crewd/crates/jev/src/decisions/route.rs`
- Modify: `crewd/crates/jev/src/lib.rs`

**Interfaces:**
- Consumes: `model::{Request, Question, ChoiceCriteria, Response, Answer}`（Task 1）、`error::CallError`（Task 3）。
- Produces:
  - `crewd_jev::shuffle`：`fn seed_from(parts: &[&str]) -> u64`、`fn shuffle<T>(items: &mut [T], seed: u64)`
  - `crewd_jev::prompts`：`struct PromptFile`（`fn get(&self, section: &str) -> &'static str`，缺节 panic 并点名；`fn section_names(&self) -> Vec<&'static str>`）、`fn parse(name: &'static str, text: &'static str) -> Result<PromptFile, String>`、静态 `ROUTE_TODO` / `WAKE` / `ATTRIBUTE` / `URGENCY` / `VOCAB: LazyLock<PromptFile>`、`const REQUIRED: &[(&str, &[&str])]`、`fn by_name(name: &str) -> Option<&'static PromptFile>`
  - `crewd_jev::decisions`：常量 `MAX_MEMBERS = 15`、`MAX_WAKE_MEMBERS = 30`、`MAX_TEXT_CHARS = 2000`、`MAX_SUMMARY_CHARS = 200`、`MAX_WORK_TITLES = 3`、`MAX_TITLE_CHARS = 80`、`MAX_RECENT = 5`、`MAX_CANDIDATES = 20`；`enum DecisionKind { RouteTodo, Wake, Attribute, Urgency }`（`ALL`、`fn as_str(self) -> &'static str`）；`struct Thresholds { choice_min: f64, score_min: f64, wake_yes: f64 }`（默认 0.5 / 0.5 / 0.5）；`enum Outcome<T> { Act(T), Defer(DeferReason) }`；`enum DeferReason { Unconfigured, InvalidInput(String), Call(CallError), Malformed(String), LowConfidence { confidence: f64 }, NobodySelected }`（`fn label(&self) -> String`）；`struct Verdict<T> { best: Option<T>, confidence: Option<f64>, outcome: Outcome<T> }`（`fn defer(reason) -> Verdict<T>`）；`struct Built { kind: DecisionKind, state: Value, questions: BTreeMap<String, Question>, options: Vec<String> }`（`fn request(&self, model: &str) -> Request`）；`struct Member { key, name, role, work: Vec<String>, busy: bool, recency_rank: usize }`（`Serialize + Deserialize`，`work` / `busy` 可省）；`fn validate_key`、`fn check_keys`、`fn clip(text: &str, max: usize) -> String`、`fn non_empty`、`fn recency_word(rank) / wait_word(Duration) / mentions_word(count) -> &'static str`、`fn most_recent(&[Member], max) -> Vec<&Member>`、`fn member_view(&Member) -> Value`、`fn read_choice(&Built, &Response, question) -> Result<(String, f64), DeferReason>`、`fn check_unit(f64) -> Result<(), DeferReason>`、`fn gate<T: Clone>(best: T, confidence: f64, min: f64) -> Verdict<T>`；测试辅助 `decisions::testing::member(key, rank) -> Member`（`#[cfg(test)] pub(crate)`）
  - `crewd_jev::decisions::route`：常量 `QUESTION_ID = "route"`、`CAPTAIN`、`NEW_SESSION`、`NEW_CREW`、`RESERVED`；`struct RouteTodoInput { todo: String, members: Vec<Member> }`；`enum RouteTarget { Captain, Member(String), NewSession, NewCrew }`（`Serialize` 成 `{"target": "...", "key": "..."}`）；`fn seed(&RouteTodoInput) -> u64`、`fn build(&RouteTodoInput, seed: u64) -> Result<Built, String>`、`fn interpret(&Built, &Response, &Thresholds) -> Verdict<RouteTarget>`、`fn target_key(&RouteTarget) -> &str`

- [ ] **Step 1: 写提示词文件**（这一步就是交付物本身，五个文件逐字照抄；措辞改动要重跑 Task 9 的评测）

`crates/jev/prompts/route_todo.md`：

```markdown
<!--
入口 Todo 派给谁（spec §7.2 第一行）。choice。
state = {"todo": 待办原文}；成员选项的描述是代码拼的对象 {name, role, work, status, activity}。
改这里（及其余 prompts/*.md）的措辞后要重跑 jev-eval（crewd/README.md「Jev 评测」），把读数记进 spec 的「M5a 交付记录与中文实测」一节。
-->

## instructions
`todo` 是人类刚交给这个机组的一条待办。从选项里选出应该接手它的那一个。只根据待办要做的事，和每个选项的职责（role）、手上的工作（work）来判断：谁手上的工作和这条待办是同一件事或直接相关，就选谁。忙闲（status）和最近活跃程度（activity）只在两个成员同样合适时用来区分，空闲的优先。

## option.captain
机长。待办需要先拆成几件事分给不同成员、在问整个机组的方向或优先级、或者要求协调多个成员时，选机长。机长也可以自己做和任何成员都不相关的小事。

## option.new_session
新开一个成员专门做。待办是一件独立的新工作：和现有每个成员手上的工作都不相关，但仍属于这个机组的项目范围，不需要新的机组。

## option.new_crew
新建一个机组。待办是一个全新的项目或长期方向，需要自己的代码仓库、工作目录或一组成员，已经超出这个机组的项目范围。
```

`crates/jev/prompts/wake.md`：

```markdown
<!--
人类消息叫醒谁（spec §7.2 第二行、§17）。群里每个成员一道 noul，指令是对象 {member, question}。
state = {"message": 人类这条消息, "recent": 之前几条群消息（旧在前）}。
只决定「谁现在醒」；可见性不变，没被叫醒的成员下次醒来照样读到。
-->

## question
`message` 是人类刚在群里发的消息，`recent` 是它之前的几条群消息。`member` 是群里的一个成员。这条消息是否需要 `member` 现在就去读并回应或动手？

## true
消息点了这个成员的名字，或者明确在对这个成员说话；或者消息在问、在要求、在纠正这个成员负责的那件工作；或者消息在回答这个成员刚才问人类的问题。

## false
消息和这个成员负责的工作无关；或者消息是对别的成员说的；或者只是打招呼、致谢、闲聊；或者这个成员只需要以后看到，不需要现在动手。
```

`crates/jev/prompts/attribute.md`：

```markdown
<!--
随手消息归属（spec §7.2 第三行、§17 大厅消息归哪个群）。choice。
state = {"message": 消息原文}；候选的描述是代码拼的对象 {kind, summary}。
-->

## instructions
`message` 是人类在群里随手发的一条消息，没有说明它是在回应什么。从选项里选出它在接着说的那一件事：它是在回应、补充或追问哪一条候选。每个候选的 kind 说明它是群消息、待办、工作项还是一个群，summary 是它的内容。

## option.none
这条消息开启了一件新的事，或者和所有候选都无关，比如打招呼、致谢、闲聊。
```

`crates/jev/prompts/urgency.md`：

```markdown
<!--
侧边栏紧急度（spec §7.2 第四行）。score，五级，从不急到最急；级别顺序就是数组顺序，绝不打乱。
state = {"crew": {name, waiting_on_human, human_mentions, issues, recent}}，时长与次数都是代码换好的词。
-->

## instructions
`crew` 是一个机组此刻的情况摘要：waiting_on_human 是成员在等人类回答的问题和已经等了多久，human_mentions 是群里叫人类的情况，issues 是系统发现的问题，recent 是最近几条群消息。这个机组有多需要人类现在就过去看？

## level.0
没有任何需要人类的事：没有在等人类的问题，没人叫人类，也没有问题，成员在正常推进或空闲。

## level.1
有人类可能想知道的进展或结果，但没有人在等人类，什么时候看都行。

## level.2
有成员在等人类回答或拍板，但只卡住一件事，其他工作还在推进，等得也不算久。

## level.3
工作卡在人类身上：关键的事停下来等人类决定、已经等了很久，或者有人反复在叫人类。

## level.4
出了严重问题需要人类立刻处理：线上故障、数据可能丢失、要花钱或涉及账号凭据的确认。
```

`crates/jev/prompts/vocab.md`：

```markdown
<!--
代码算好的量换成的词（jaggedness #2/#3：时间、计数、排名不给 Jev 原始数字）。
分档边界在 src/decisions/mod.rs（recency_word / wait_word / mentions_word），改档位要同时改那里的测试。
-->

## status.busy
正在忙

## status.idle
空闲

## recency.recent
刚刚还在活跃

## recency.middle
不久前活跃过

## recency.stale
很久没有活跃

## kind.message
群消息

## kind.todo
待办

## kind.work_item
工作项

## kind.channel
群

## wait.minutes
刚问不久

## wait.hours
等了几个小时

## wait.day
等了大半天到一天

## wait.days
等了好几天

## mentions.none
没人叫人类

## mentions.some
有人叫过人类

## mentions.many
好几次在叫人类
```

- [ ] **Step 2: 写失败的测试**

`lib.rs`：

```rust
pub mod client;
pub mod decisions;
pub mod error;
pub mod key;
pub mod model;
pub mod prompts;
pub mod scrub;
pub mod shuffle;
```

`crates/jev/src/shuffle.rs`、`crates/jev/src/prompts.rs`、`crates/jev/src/decisions/mod.rs`、`crates/jev/src/decisions/route.rs`：各自先只放测试部分。

`shuffle.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_order_and_it_is_a_permutation() {
        let mut a: Vec<u32> = (0..10).collect();
        let mut b = a.clone();
        shuffle(&mut a, 42);
        shuffle(&mut b, 42);
        assert_eq!(a, b);
        let mut sorted = a.clone();
        sorted.sort();
        assert_eq!(sorted, (0..10).collect::<Vec<_>>());
        assert_ne!(a, (0..10).collect::<Vec<_>>(), "种子 42 不该恰好不动");
    }

    #[test]
    fn every_item_leads_about_equally_often() {
        let n = 6;
        let rounds = 6000;
        let mut first = vec![0usize; n];
        for seed in 0..rounds {
            let mut v: Vec<usize> = (0..n).collect();
            shuffle(&mut v, seed_from(&["route_todo", &seed.to_string()]));
            first[v[0]] += 1;
        }
        for (item, count) in first.iter().enumerate() {
            assert!(
                (800..=1200).contains(count),
                "选项 {item} 排第一 {count} 次（期望约 1000）：{first:?}"
            );
        }
    }

    #[test]
    fn seed_separates_segments() {
        assert_ne!(seed_from(&["ab", "c"]), seed_from(&["a", "bc"]));
        assert_ne!(seed_from(&["x"]), seed_from(&["x", ""]));
        assert_eq!(seed_from(&["修", "滚动"]), seed_from(&["修", "滚动"]));
    }

    #[test]
    fn tiny_inputs_are_fine() {
        let mut empty: Vec<u8> = vec![];
        shuffle(&mut empty, 1);
        let mut one = vec![7];
        shuffle(&mut one, 1);
        assert_eq!(one, [7]);
    }
}
```

`prompts.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prompt_file_has_its_sections() {
        for (name, required) in REQUIRED {
            let file = by_name(name).unwrap_or_else(|| panic!("REQUIRED 里有未知文件 {name}"));
            for section in *required {
                assert!(
                    file.section_names().contains(section),
                    "prompts/{name}.md 缺 ## {section}"
                );
            }
        }
    }

    #[test]
    fn prompt_texts_carry_no_digits_that_could_be_mistaken_for_data() {
        // jaggedness #2/#3：数字、日期交给代码。提示词自己也不写数字，免得被当成状态里的事实。
        for (name, required) in REQUIRED {
            let file = by_name(name).unwrap();
            for section in *required {
                let text = file.get(section);
                assert!(
                    !text.chars().any(|c| c.is_ascii_digit()),
                    "prompts/{name}.md ## {section} 含数字：{text}"
                );
            }
        }
    }

    #[test]
    fn parser_rejects_empty_and_duplicate_sections() {
        assert!(parse("t", "## a\n\n## b\nx\n").is_err());
        assert!(parse("t", "## a\nx\n## a\ny\n").is_err());
        assert!(parse("t", "## \nx\n").is_err());
        let ok = parse("t", "注释\n## a\n第一行\n第二行\n\n## b\ny").unwrap();
        assert_eq!(ok.get("a"), "第一行\n第二行");
        assert_eq!(ok.get("b"), "y");
        assert_eq!(ok.section_names(), ["a", "b"]);
    }

    #[test]
    #[should_panic(expected = "prompts/t.md 缺少一节：## nope")]
    fn missing_section_names_itself() {
        parse("t", "## a\nx").unwrap().get("nope");
    }
}
```

`decisions/mod.rs`（注意：这一任务只声明 `route` 一个子模块；测试辅助 `testing` 也在这里）：

```rust
pub mod route;

#[cfg(test)]
pub(crate) mod testing {
    use super::Member;

    pub fn member(key: &str, rank: usize) -> Member {
        Member {
            key: key.into(),
            name: format!("成员{key}"),
            role: format!("负责 {key}"),
            work: vec![format!("{key} 的工作")],
            busy: false,
            recency_rank: rank,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::member;
    use super::*;

    #[test]
    fn defaults_are_pinned() {
        assert_eq!(
            Thresholds::default(),
            Thresholds {
                choice_min: 0.5,
                score_min: 0.5,
                wake_yes: 0.5
            }
        );
    }

    #[test]
    fn keys_must_be_ascii_slugs_unique_and_not_reserved() {
        assert!(validate_key("scroll-fix").is_ok());
        assert!(validate_key("w3_b").is_ok());
        assert!(validate_key("").is_err());
        assert!(validate_key("Scroll").is_err());
        assert!(validate_key("-x").is_err());
        assert!(validate_key("滚动").is_err());
        assert!(validate_key(&"a".repeat(33)).is_err());
        assert!(check_keys(["a", "b"], &["captain"]).is_ok());
        assert!(check_keys(["a", "a"], &[]).is_err());
        assert!(check_keys(["captain"], &["captain"]).is_err());
    }

    #[test]
    fn clip_counts_characters_not_bytes() {
        assert_eq!(clip("  修滚动  ", 10), "修滚动");
        assert_eq!(clip("一二三四五", 3), "一二…");
        assert_eq!(clip("abc", 3), "abc");
    }

    #[test]
    fn quantities_become_words_without_digits() {
        assert_eq!(recency_word(1), VOCAB.get("recency.recent"));
        assert_eq!(recency_word(3), VOCAB.get("recency.recent"));
        assert_eq!(recency_word(4), VOCAB.get("recency.middle"));
        assert_eq!(recency_word(8), VOCAB.get("recency.middle"));
        assert_eq!(recency_word(9), VOCAB.get("recency.stale"));
        assert_eq!(recency_word(0), VOCAB.get("recency.stale"));
        let m = Duration::from_secs(60);
        assert_eq!(wait_word(m * 59), VOCAB.get("wait.minutes"));
        assert_eq!(wait_word(m * 60), VOCAB.get("wait.hours"));
        assert_eq!(wait_word(m * 479), VOCAB.get("wait.hours"));
        assert_eq!(wait_word(m * 480), VOCAB.get("wait.day"));
        assert_eq!(wait_word(m * 36 * 60), VOCAB.get("wait.days"));
        assert_eq!(mentions_word(0), VOCAB.get("mentions.none"));
        assert_eq!(mentions_word(2), VOCAB.get("mentions.some"));
        assert_eq!(mentions_word(3), VOCAB.get("mentions.many"));
    }

    #[test]
    fn roster_is_cut_to_the_most_recent_and_unknown_rank_goes_last() {
        let mut members: Vec<Member> = (1..=20)
            .rev()
            .map(|r| member(&format!("m{r}"), r))
            .collect();
        members.push(member("unknown", 0));
        let top = most_recent(&members, MAX_MEMBERS);
        assert_eq!(top.len(), 15);
        assert_eq!(top[0].key, "m1");
        assert_eq!(top[14].key, "m15");
        assert!(top.iter().all(|m| m.key != "unknown"));
    }

    #[test]
    fn member_view_has_no_raw_numbers() {
        let mut m = member("a", 2);
        m.work = vec!["一".into(), "二".into(), "三".into(), "四".into()];
        m.busy = true;
        let v = member_view(&m);
        assert_eq!(v["work"].as_array().unwrap().len(), MAX_WORK_TITLES);
        assert_eq!(v["status"], VOCAB.get("status.busy"));
        assert_eq!(v["activity"], VOCAB.get("recency.recent"));
        assert!(!v.to_string().contains("recency_rank"));
    }
}
```

`decisions/route.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::decisions::testing::member;
    use crate::decisions::{DeferReason, Outcome};
    use crate::model::Answer;

    fn input(n: usize) -> RouteTodoInput {
        RouteTodoInput {
            todo: "  修一下终端滚动跳动  ".into(),
            members: (1..=n).map(|r| member(&format!("m{r}"), r)).collect(),
        }
    }

    fn answer(choice: &str, confidence: f64) -> Response {
        let mut answers = BTreeMap::new();
        answers.insert(
            QUESTION_ID.to_string(),
            Answer::Choice {
                choice: choice.into(),
                probabilities: BTreeMap::new(),
                confidence,
            },
        );
        Response {
            model: "jev-1.13.0".into(),
            answers,
            usage: None,
        }
    }

    #[test]
    fn options_are_the_three_fixed_ones_plus_members_shuffled_by_seed() {
        let a = build(&input(4), 1).unwrap();
        let b = build(&input(4), 1).unwrap();
        assert_eq!(a, b, "同一种子同一请求");
        let mut sorted = a.options.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            ["captain", "m1", "m2", "m3", "m4", "new_crew", "new_session"]
        );
        let firsts: std::collections::BTreeSet<String> = (0..40)
            .map(|s| build(&input(4), s).unwrap().options[0].clone())
            .collect();
        assert!(firsts.len() >= 5, "打乱后排第一的选项应该轮换：{firsts:?}");
        assert_eq!(a.state, json!({"todo": "修一下终端滚动跳动"}));
    }

    #[test]
    fn wire_order_matches_options() {
        let built = build(&input(3), 7).unwrap();
        // 必须直接序列化成文本：serde_json::Value 的对象是按键排序的，转一道 Value 顺序就没了。
        let text = serde_json::to_string(&built.request("jev-latest")).unwrap();
        let mut last = 0;
        for key in &built.options {
            let pos = text.find(&format!("\"{key}\":")).unwrap();
            assert!(pos >= last, "选项 {key} 在线上的位置乱了");
            last = pos;
        }
    }

    #[test]
    fn roster_is_capped_at_fifteen_most_recent() {
        let built = build(&input(25), 3).unwrap();
        assert_eq!(built.options.len(), 3 + MAX_MEMBERS);
        assert!(built.options.contains(&"m15".to_string()));
        assert!(!built.options.contains(&"m16".to_string()));
    }

    #[test]
    fn bad_inputs_are_refused() {
        let mut i = input(2);
        i.todo = "   ".into();
        assert!(build(&i, 0).is_err());
        let mut i = input(2);
        i.members[0].key = "captain".into();
        assert!(build(&i, 0).is_err());
        let mut i = input(2);
        i.members[1].key = "m1".into();
        assert!(build(&i, 0).is_err());
    }

    #[test]
    fn answers_map_to_targets_and_low_confidence_defers() {
        let built = build(&input(2), 0).unwrap();
        let th = Thresholds::default();
        let v = interpret(&built, &answer("m2", 0.9), &th);
        assert_eq!(v.outcome, Outcome::Act(RouteTarget::Member("m2".into())));
        let v = interpret(&built, &answer("new_crew", 0.8), &th);
        assert_eq!(v.outcome, Outcome::Act(RouteTarget::NewCrew));
        let v = interpret(&built, &answer("captain", 0.2), &th);
        assert_eq!(v.best, Some(RouteTarget::Captain), "评测要看到模型的答案");
        assert_eq!(
            v.outcome,
            Outcome::Defer(DeferReason::LowConfidence { confidence: 0.2 })
        );
    }

    #[test]
    fn invented_or_missing_answers_are_malformed() {
        let built = build(&input(2), 0).unwrap();
        let th = Thresholds::default();
        assert!(matches!(
            interpret(&built, &answer("m9", 0.99), &th).outcome,
            Outcome::Defer(DeferReason::Malformed(_))
        ));
        assert!(matches!(
            interpret(&built, &answer("m1", 1.5), &th).outcome,
            Outcome::Defer(DeferReason::Malformed(_))
        ));
        let empty = Response {
            model: "m".into(),
            answers: BTreeMap::new(),
            usage: None,
        };
        assert!(matches!(
            interpret(&built, &empty, &th).outcome,
            Outcome::Defer(DeferReason::Malformed(_))
        ));
    }
}
```

- [ ] **Step 3: 跑一遍确认失败**

Run: `cargo test -p crewd-jev --lib`
Expected: 编译失败（`shuffle`、`parse`、`Member`、`build` 等未定义）。

- [ ] **Step 4: 写实现**

`shuffle.rs`（测试模块上面）：

```rust
//! 确定性打乱：jev-1.13 偏向第一个选项（官方 jaggedness #8），每次判断把选项顺序打乱。
//! 种子由输入算出（同一输入同一顺序，诊断日志可复现），不引入 rand。
//! SplitMix64（Steele 等，2014）+ Fisher–Yates。

/// FNV-1a 64 位，段与段之间插一个 0xff（UTF-8 里不会出现的字节），避免 ["ab","c"] 与 ["a","bc"] 撞。
pub fn seed_from(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.bytes().chain(std::iter::once(0xff)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// [0, n)。n ≤ 255（官方选项上限），取模偏差可忽略。
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

pub fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut rng = SplitMix64(seed);
    for i in (1..items.len()).rev() {
        let j = rng.below(i + 1);
        items.swap(i, j);
    }
}
```

`prompts.rs`（测试模块上面）：

```rust
//! 给 Jev 看的文字全在 `prompts/*.md`（编译期 include_str!），代码里不写问题 / 说明 / 选项描述的字面量。
//! 文件格式：第一个 `## 名字` 之前的内容是给人看的注释，忽略；之后每个 `## 名字` 开一节，
//! 节内容 = 到下一个 `## ` 为止的文本去掉首尾空白。节名重复、节内容为空都是错误。
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Debug)]
pub struct PromptFile {
    pub name: &'static str,
    sections: BTreeMap<&'static str, &'static str>,
}

impl PromptFile {
    /// 缺节 = 程序员错误（`every_prompt_file_has_its_sections` 测试钉住），直接 panic 并说清是哪一节。
    pub fn get(&self, section: &str) -> &'static str {
        match self.sections.get(section) {
            Some(text) => text,
            None => panic!("prompts/{}.md 缺少一节：## {section}", self.name),
        }
    }

    pub fn section_names(&self) -> Vec<&'static str> {
        self.sections.keys().copied().collect()
    }
}

pub fn parse(name: &'static str, text: &'static str) -> Result<PromptFile, String> {
    let mut sections = BTreeMap::new();
    let mut current: Option<(&'static str, usize)> = None;
    let mut offset = 0;
    let mut close = |cur: Option<(&'static str, usize)>, end: usize| -> Result<(), String> {
        if let Some((key, start)) = cur {
            let body = text[start..end].trim();
            if body.is_empty() {
                return Err(format!("prompts/{name}.md 的 ## {key} 是空的"));
            }
            if sections.insert(key, body).is_some() {
                return Err(format!("prompts/{name}.md 的 ## {key} 重复"));
            }
        }
        Ok(())
    };
    for line in text.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("## ") {
            close(current.take(), offset)?;
            let key = rest.trim();
            if key.is_empty() {
                return Err(format!("prompts/{name}.md 有一个没有名字的 ##"));
            }
            current = Some((key, offset + line.len()));
        }
        offset += line.len();
    }
    close(current, text.len())?;
    Ok(PromptFile { name, sections })
}

fn load(name: &'static str, text: &'static str) -> PromptFile {
    parse(name, text).unwrap_or_else(|e| panic!("{e}"))
}

pub static ROUTE_TODO: LazyLock<PromptFile> =
    LazyLock::new(|| load("route_todo", include_str!("../prompts/route_todo.md")));
pub static WAKE: LazyLock<PromptFile> =
    LazyLock::new(|| load("wake", include_str!("../prompts/wake.md")));
pub static ATTRIBUTE: LazyLock<PromptFile> =
    LazyLock::new(|| load("attribute", include_str!("../prompts/attribute.md")));
pub static URGENCY: LazyLock<PromptFile> =
    LazyLock::new(|| load("urgency", include_str!("../prompts/urgency.md")));
pub static VOCAB: LazyLock<PromptFile> =
    LazyLock::new(|| load("vocab", include_str!("../prompts/vocab.md")));

/// 每个文件必须有的节。决策构造器只读这里列出的节。
pub const REQUIRED: &[(&str, &[&str])] = &[
    (
        "route_todo",
        &[
            "instructions",
            "option.captain",
            "option.new_session",
            "option.new_crew",
        ],
    ),
    ("wake", &["question", "true", "false"]),
    ("attribute", &["instructions", "option.none"]),
    (
        "urgency",
        &[
            "instructions",
            "level.0",
            "level.1",
            "level.2",
            "level.3",
            "level.4",
        ],
    ),
    (
        "vocab",
        &[
            "status.busy",
            "status.idle",
            "recency.recent",
            "recency.middle",
            "recency.stale",
            "kind.message",
            "kind.todo",
            "kind.work_item",
            "kind.channel",
            "wait.minutes",
            "wait.hours",
            "wait.day",
            "wait.days",
            "mentions.none",
            "mentions.some",
            "mentions.many",
        ],
    ),
];

pub fn by_name(name: &str) -> Option<&'static PromptFile> {
    match name {
        "route_todo" => Some(&ROUTE_TODO),
        "wake" => Some(&WAKE),
        "attribute" => Some(&ATTRIBUTE),
        "urgency" => Some(&URGENCY),
        "vocab" => Some(&VOCAB),
        _ => None,
    }
}
```

`decisions/mod.rs`（整个实现部分，放在测试辅助与测试模块上面；替换掉 Step 2 里那行单独的 `pub mod route;`）：

```rust
//! 四个决策（spec §7.2）的共用部分：输入里的成员、阈值、构造结果 `Built`、判定 `Verdict` / `Outcome`，
//! 以及「代码算好的量 → 词」的分档（jaggedness #2/#3：不给 Jev 日期、时长、计数）。
//! 构造器是纯函数：同一输入 + 同一种子 → 同一请求。不碰网络、不读时钟。
pub mod route;

use crate::error::CallError;
use crate::model::{Answer, Question, Request, Response};
use crate::prompts::VOCAB;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

/// 名单截到最近活跃的前 15 个（spec §7.3「名单截到最近活跃前 ~15」）。
pub const MAX_MEMBERS: usize = 15;
/// 叫醒是每人一道 noul，不挤在一个选项表里，上限放宽，但仍要有界。
pub const MAX_WAKE_MEMBERS: usize = 30;
pub const MAX_TEXT_CHARS: usize = 2000;
pub const MAX_SUMMARY_CHARS: usize = 200;
pub const MAX_WORK_TITLES: usize = 3;
pub const MAX_TITLE_CHARS: usize = 80;
pub const MAX_RECENT: usize = 5;
pub const MAX_CANDIDATES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    RouteTodo,
    Wake,
    Attribute,
    Urgency,
}

impl DecisionKind {
    pub const ALL: [DecisionKind; 4] = [
        DecisionKind::RouteTodo,
        DecisionKind::Wake,
        DecisionKind::Attribute,
        DecisionKind::Urgency,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DecisionKind::RouteTodo => "route_todo",
            DecisionKind::Wake => "wake",
            DecisionKind::Attribute => "attribute",
            DecisionKind::Urgency => "urgency",
        }
    }
}

/// 置信度线。默认值 = spec §7.3 的 0.5；M5a Task 9 实测后按读数改（改了要同步 `defaults_are_pinned`）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    /// choice 的 confidence 低于它 → 交机长。
    pub choice_min: f64,
    /// score 的 confidence 低于它 → 侧边栏退回只按最近活跃排。
    pub score_min: f64,
    /// noul 的 p 不低于它 → 叫醒这个成员。
    pub wake_yes: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            choice_min: 0.5,
            score_min: 0.5,
            wake_yes: 0.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome<T> {
    /// 中 / 高置信度：直接执行，群里不留痕（spec §7.1）。
    Act(T),
    /// 交机长按老办法处理（或侧边栏退回代码排序）。
    Defer(DeferReason),
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeferReason {
    /// 没开 / 没 key：等同低置信度（spec §7.1）。
    Unconfigured,
    /// 输入本身不能问（key 不合法、重复、空文本……）。是调用方的错，诊断日志里能看到。
    InvalidInput(String),
    Call(CallError),
    /// 应答缺题、类型不对、选了不存在的选项、置信度越界。
    Malformed(String),
    LowConfidence {
        confidence: f64,
    },
    /// 叫醒集合为空：交机长（spec §7.2）。
    NobodySelected,
}

impl DeferReason {
    pub fn label(&self) -> String {
        match self {
            DeferReason::Unconfigured => "unconfigured".into(),
            DeferReason::InvalidInput(m) => format!("invalid_input: {m}"),
            DeferReason::Call(e) => format!("call: {e}"),
            DeferReason::Malformed(m) => format!("malformed: {m}"),
            DeferReason::LowConfidence { confidence } => {
                format!("low_confidence: {confidence:.3}")
            }
            DeferReason::NobodySelected => "nobody_selected".into(),
        }
    }
}

/// 一次判断的结论。`best` 是模型给出的答案（不论置信度够不够），评测靠它算准确率；
/// 执行只看 `outcome`。
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict<T> {
    pub best: Option<T>,
    pub confidence: Option<f64>,
    pub outcome: Outcome<T>,
}

impl<T> Verdict<T> {
    pub fn defer(reason: DeferReason) -> Verdict<T> {
        Verdict {
            best: None,
            confidence: None,
            outcome: Outcome::Defer(reason),
        }
    }
}

/// 构造好的一次判断：发给 Jev 的 state + questions，以及按发出顺序排的选项名（choice）或题号（noul）。
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub kind: DecisionKind,
    pub state: Value,
    pub questions: BTreeMap<String, Question>,
    pub options: Vec<String>,
}

impl Built {
    pub fn request(&self, model: &str) -> Request {
        Request {
            model: model.to_string(),
            state: self.state.clone(),
            questions: self.questions.clone(),
        }
    }
}

/// 群成员 / 活跃 session 的一行。`key` 是选项名（发给模型看），必须是 `[a-z0-9][a-z0-9_-]{0,31}`；
/// `recency_rank` 从 1 起，1 = 最近活跃，代码算；给 Jev 的只是分档词。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub key: String,
    pub name: String,
    pub role: String,
    #[serde(default)]
    pub work: Vec<String>,
    #[serde(default)]
    pub busy: bool,
    pub recency_rank: usize,
}

pub fn validate_key(key: &str) -> Result<(), String> {
    let bytes = key.as_bytes();
    let ok = !bytes.is_empty()
        && bytes.len() <= 32
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!("选项名不合法：{key:?}"))
    }
}

/// 校验一组选项名：合法、互不重复、不撞保留名。
pub fn check_keys<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    reserved: &[&str],
) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for key in keys {
        validate_key(key)?;
        if reserved.contains(&key) {
            return Err(format!("选项名 {key:?} 是保留名"));
        }
        if !seen.insert(key) {
            return Err(format!("选项名 {key:?} 重复"));
        }
    }
    Ok(())
}

/// 去首尾空白，超过 `max` 个字符截断并补「…」。
pub fn clip(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let mut out: String = t.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn non_empty(text: &str, what: &str) -> Result<String, String> {
    let t = clip(text, MAX_TEXT_CHARS);
    if t.is_empty() {
        Err(format!("{what}是空的"))
    } else {
        Ok(t)
    }
}

/// 最近活跃排名 → 词。1–3 刚刚、4–8 不久前、其余（含 0 = 不知道）很久。
pub fn recency_word(rank: usize) -> &'static str {
    match rank {
        1..=3 => VOCAB.get("recency.recent"),
        4..=8 => VOCAB.get("recency.middle"),
        _ => VOCAB.get("recency.stale"),
    }
}

/// 已等时长 → 词。< 1h、< 8h、< 36h、更久。
pub fn wait_word(waited: Duration) -> &'static str {
    let mins = waited.as_secs() / 60;
    if mins < 60 {
        VOCAB.get("wait.minutes")
    } else if mins < 8 * 60 {
        VOCAB.get("wait.hours")
    } else if mins < 36 * 60 {
        VOCAB.get("wait.day")
    } else {
        VOCAB.get("wait.days")
    }
}

/// 叫人类的次数 → 词。0、1–2、≥3。
pub fn mentions_word(count: usize) -> &'static str {
    match count {
        0 => VOCAB.get("mentions.none"),
        1 | 2 => VOCAB.get("mentions.some"),
        _ => VOCAB.get("mentions.many"),
    }
}

/// 按最近活跃排序（0 = 不知道，排最后；同名次保持输入顺序），截前 `max` 个。
pub fn most_recent(members: &[Member], max: usize) -> Vec<&Member> {
    let mut sorted: Vec<&Member> = members.iter().collect();
    sorted.sort_by_key(|m| {
        if m.recency_rank == 0 {
            usize::MAX
        } else {
            m.recency_rank
        }
    });
    sorted.truncate(max);
    sorted
}

/// 给模型看的成员描述。字段名是英文结构名，值是数据或 vocab 里的词。
pub fn member_view(m: &Member) -> Value {
    let work: Vec<String> = m
        .work
        .iter()
        .map(|w| clip(w, MAX_TITLE_CHARS))
        .filter(|w| !w.is_empty())
        .take(MAX_WORK_TITLES)
        .collect();
    json!({
        "name": clip(&m.name, MAX_TITLE_CHARS),
        "role": clip(&m.role, MAX_SUMMARY_CHARS),
        "work": work,
        "status": if m.busy { VOCAB.get("status.busy") } else { VOCAB.get("status.idle") },
        "activity": recency_word(m.recency_rank),
    })
}

/// 取一道 choice 的答案并核对：选项必须是我们发出去的，置信度必须在 [0,1]。
pub fn read_choice(
    built: &Built,
    response: &Response,
    question: &str,
) -> Result<(String, f64), DeferReason> {
    match response.answers.get(question) {
        Some(Answer::Choice {
            choice, confidence, ..
        }) => {
            if !built.options.iter().any(|o| o == choice) {
                return Err(DeferReason::Malformed(format!(
                    "选了没发出的选项 {choice:?}"
                )));
            }
            check_unit(*confidence)?;
            Ok((choice.clone(), *confidence))
        }
        Some(other) => Err(DeferReason::Malformed(format!(
            "{question} 的答案类型不对：{other:?}"
        ))),
        None => Err(DeferReason::Malformed(format!("应答里没有 {question}"))),
    }
}

pub fn check_unit(x: f64) -> Result<(), DeferReason> {
    if (0.0..=1.0).contains(&x) {
        Ok(())
    } else {
        Err(DeferReason::Malformed(format!("越界的概率 / 置信度 {x}")))
    }
}

/// 置信度够线就执行，否则交机长；`best` 无论如何都留着给评测。
pub fn gate<T: Clone>(best: T, confidence: f64, min: f64) -> Verdict<T> {
    let outcome = if confidence >= min {
        Outcome::Act(best.clone())
    } else {
        Outcome::Defer(DeferReason::LowConfidence { confidence })
    };
    Verdict {
        best: Some(best),
        confidence: Some(confidence),
        outcome,
    }
}
```

`decisions/route.rs`（测试模块上面）：

```rust
//! 入口 Todo 派给谁（spec §7.2）：一道 choice，选项 = 机长 / 每个活跃 session / 新开 session / 新建机组。
//! 选项顺序按种子打乱（jaggedness #8）。new_crew 由调用方交机长执行（建组要起名，Jev 不生成文字）。
use super::{
    Built, DecisionKind, MAX_MEMBERS, Member, Thresholds, Verdict, check_keys, gate, member_view,
    most_recent, non_empty, read_choice,
};
use crate::model::{ChoiceCriteria, Question, Response};
use crate::prompts::ROUTE_TODO;
use crate::shuffle::{seed_from, shuffle};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const QUESTION_ID: &str = "route";
pub const CAPTAIN: &str = "captain";
pub const NEW_SESSION: &str = "new_session";
pub const NEW_CREW: &str = "new_crew";
pub const RESERVED: [&str; 3] = [CAPTAIN, NEW_SESSION, NEW_CREW];

#[derive(Debug, Clone, PartialEq)]
pub struct RouteTodoInput {
    /// 入口 Todo 原文。
    pub todo: String,
    /// 活跃 session（不含机长；机长是固定选项）。
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "target", content = "key", rename_all = "snake_case")]
pub enum RouteTarget {
    Captain,
    Member(String),
    NewSession,
    NewCrew,
}

pub fn seed(input: &RouteTodoInput) -> u64 {
    seed_from(&[DecisionKind::RouteTodo.as_str(), &input.todo])
}

pub fn build(input: &RouteTodoInput, seed: u64) -> Result<Built, String> {
    let todo = non_empty(&input.todo, "待办")?;
    let members = most_recent(&input.members, MAX_MEMBERS);
    check_keys(members.iter().map(|m| m.key.as_str()), &RESERVED)?;
    let mut options: Vec<(String, Value)> = vec![
        (CAPTAIN.into(), json!(ROUTE_TODO.get("option.captain"))),
        (
            NEW_SESSION.into(),
            json!(ROUTE_TODO.get("option.new_session")),
        ),
        (NEW_CREW.into(), json!(ROUTE_TODO.get("option.new_crew"))),
    ];
    options.extend(members.iter().map(|m| (m.key.clone(), member_view(m))));
    shuffle(&mut options, seed);
    let criteria = ChoiceCriteria(options);
    let keys = criteria.keys();
    let mut questions = BTreeMap::new();
    questions.insert(
        QUESTION_ID.to_string(),
        Question::Choice {
            instructions: json!(ROUTE_TODO.get("instructions")),
            criteria,
        },
    );
    Ok(Built {
        kind: DecisionKind::RouteTodo,
        state: json!({ "todo": todo }),
        questions,
        options: keys,
    })
}

pub fn interpret(
    built: &Built,
    response: &Response,
    thresholds: &Thresholds,
) -> Verdict<RouteTarget> {
    let (key, confidence) = match read_choice(built, response, QUESTION_ID) {
        Ok(v) => v,
        Err(reason) => return Verdict::defer(reason),
    };
    let target = match key.as_str() {
        CAPTAIN => RouteTarget::Captain,
        NEW_SESSION => RouteTarget::NewSession,
        NEW_CREW => RouteTarget::NewCrew,
        _ => RouteTarget::Member(key),
    };
    gate(target, confidence, thresholds.choice_min)
}

/// 评测与诊断用：从 `Verdict` 里拿到选项名（与数据集里的期望同一套词）。
pub fn target_key(target: &RouteTarget) -> &str {
    match target {
        RouteTarget::Captain => CAPTAIN,
        RouteTarget::NewSession => NEW_SESSION,
        RouteTarget::NewCrew => NEW_CREW,
        RouteTarget::Member(k) => k,
    }
}
```

- [ ] **Step 5: 跑通**

Run: `cargo test -p crewd-jev --lib`
Expected: `40 passed; 1 ignored`（Task 4 的 20 + shuffle 4 + prompts 4 + decisions 6 + route 6）。

- [ ] **Step 6: 变异核对**

逐条做、每条撤销：
1. `route::build` 里删掉 `shuffle(&mut options, seed);`（换成 `let _ = seed;`）→ Expected `options_are_the_three_fixed_ones_plus_members_shuffled_by_seed` 红（写计划时核过）。
2. `ChoiceCriteria` 的 `serialize` 改成先按键排序再写 → Expected `choice_criteria_keep_their_order_on_the_wire` 与 `wire_order_matches_options` 红（写计划时核过）。
3. 在 `prompts/vocab.md` 的 `## wait.days` 正文里加一个「3」→ Expected `prompt_texts_carry_no_digits_that_could_be_mistaken_for_data` 红。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/jev/prompts/route_todo.md crates/jev/prompts/wake.md crates/jev/prompts/attribute.md crates/jev/prompts/urgency.md crates/jev/prompts/vocab.md crates/jev/src/prompts.rs crates/jev/src/shuffle.rs crates/jev/src/decisions/mod.rs crates/jev/src/decisions/route.rs crates/jev/src/lib.rs
git diff --cached --stat
git commit -m "crewd-jev: prompt resource files, seeded option shuffle, decision core and todo routing

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: 叫醒、归属、紧急度（`decisions::{wake, attribute, urgency}`）

规则见裁定 10–12。三者共用 Task 5 的公共部分；`wake` 与 `urgency` 不打乱（noul 每人一题、score 级别有序），所以 `build` 不收种子。

**Files:**
- Create: `crewd/crates/jev/src/decisions/wake.rs`、`crewd/crates/jev/src/decisions/attribute.rs`、`crewd/crates/jev/src/decisions/urgency.rs`
- Modify: `crewd/crates/jev/src/decisions/mod.rs`（顶部模块声明）

**Interfaces:**
- Consumes: Task 5 的 `decisions::*`、`prompts::{WAKE, ATTRIBUTE, URGENCY, VOCAB}`、`shuffle::*`；`model::{Answer, NoulCriteria, noul_confidence}`。
- Produces:
  - `decisions::wake`：`struct WakeInput { message: String, recent: Vec<String>, members: Vec<Member> }`；`struct WakeSet { wake: Vec<String> }`；`fn build(&WakeInput) -> Result<Built, String>`；`fn probabilities(&Built, &Response) -> Result<Vec<(String, f64)>, DeferReason>`；`fn interpret(&Built, &Response, &Thresholds) -> Verdict<WakeSet>`
  - `decisions::attribute`：常量 `QUESTION_ID = "attribute"`、`NONE = "none"`；`enum CandidateKind { Message, Todo, WorkItem, Channel }`（serde snake_case）；`struct Candidate { key, kind, summary }`（`Serialize + Deserialize`）；`struct AttributeInput { message, candidates }`；`enum Attribution { Candidate(String), None }`；`fn seed`、`fn build(&AttributeInput, seed) -> Result<Built, String>`、`fn interpret(...) -> Verdict<Attribution>`、`fn attribution_key(&Attribution) -> &str`
  - `decisions::urgency`：常量 `QUESTION_ID = "urgency"`、`LEVELS = 5`、`MAX_WAITING = 8`；`struct HumanRequest { text: String, waited: Duration }`；`struct UrgencyInput { crew_name, waiting_on_human: Vec<HumanRequest>, human_mentions: usize, issues: Vec<String>, recent: Vec<String> }`；`struct Urgency { score: f64, level: u8 }`；`fn build(&UrgencyInput) -> Result<Built, String>`、`fn interpret(...) -> Verdict<Urgency>`

- [ ] **Step 1: 写失败的测试**

`decisions/mod.rs` 顶部的模块声明改成：

```rust
pub mod attribute;
pub mod route;
pub mod urgency;
pub mod wake;
```

`decisions/wake.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::decisions::testing::member;

    fn input() -> WakeInput {
        WakeInput {
            message: "@发版 0.1.40 的 dmg 能出了吗".into(),
            recent: (1..=8).map(|i| format!("旧消息{i}")).collect(),
            members: vec![
                member("captain", 2),
                member("release", 1),
                member("scroll", 3),
            ],
        }
    }

    fn nouls(ps: &[(&str, f64)]) -> Response {
        Response {
            model: "jev-1.13.0".into(),
            answers: ps
                .iter()
                .map(|(k, p)| (k.to_string(), Answer::Noul { noul: *p }))
                .collect(),
            usage: None,
        }
    }

    #[test]
    fn one_noul_per_member_and_only_the_last_five_recent_messages() {
        let built = build(&input()).unwrap();
        assert_eq!(built.options, ["release", "captain", "scroll"]);
        assert_eq!(built.questions.len(), 3);
        assert_eq!(
            built.state["recent"],
            json!(["旧消息4", "旧消息5", "旧消息6", "旧消息7", "旧消息8"])
        );
        let q = serde_json::to_value(&built.questions["release"]).unwrap();
        assert_eq!(q["type"], "noul");
        assert_eq!(q["instructions"]["member"]["name"], "成员release");
        assert_eq!(q["instructions"]["question"], WAKE.get("question"));
    }

    #[test]
    fn wake_set_is_everyone_at_or_above_the_line() {
        let built = build(&input()).unwrap();
        let v = interpret(
            &built,
            &nouls(&[("release", 0.97), ("captain", 0.5), ("scroll", 0.1)]),
            &Thresholds::default(),
        );
        let set = WakeSet {
            wake: vec!["release".into(), "captain".into()],
        };
        assert_eq!(v.outcome, Outcome::Act(set));
        assert!(
            (v.confidence.unwrap() - 0.0).abs() < 1e-9,
            "captain 恰好 0.5，最没把握"
        );
    }

    #[test]
    fn empty_set_goes_to_the_captain() {
        let built = build(&input()).unwrap();
        let v = interpret(
            &built,
            &nouls(&[("release", 0.2), ("captain", 0.1), ("scroll", 0.05)]),
            &Thresholds::default(),
        );
        assert_eq!(v.outcome, Outcome::Defer(DeferReason::NobodySelected));
        assert_eq!(v.best, Some(WakeSet { wake: vec![] }));
    }

    #[test]
    fn a_missing_member_answer_voids_the_whole_judgement() {
        let built = build(&input()).unwrap();
        let v = interpret(
            &built,
            &nouls(&[("release", 0.9), ("captain", 0.1)]),
            &Thresholds::default(),
        );
        assert!(matches!(
            v.outcome,
            Outcome::Defer(DeferReason::Malformed(_))
        ));
    }

    #[test]
    fn no_members_or_empty_message_is_refused() {
        let mut i = input();
        i.members.clear();
        assert!(build(&i).is_err());
        let mut i = input();
        i.message = "\n".into();
        assert!(build(&i).is_err());
    }
}
```

`decisions/attribute.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::decisions::Outcome;
    use crate::model::Answer;

    fn input(n: usize) -> AttributeInput {
        AttributeInput {
            message: "那个还是不行".into(),
            candidates: (1..=n)
                .map(|i| Candidate {
                    key: format!("c{i}"),
                    kind: if i % 2 == 0 {
                        CandidateKind::Todo
                    } else {
                        CandidateKind::Message
                    },
                    summary: format!("候选{i}"),
                })
                .collect(),
        }
    }

    #[test]
    fn candidates_plus_none_capped_and_described_with_vocab_words() {
        let built = build(&input(25), 5).unwrap();
        assert_eq!(built.options.len(), MAX_CANDIDATES + 1);
        assert!(built.options.contains(&NONE.to_string()));
        assert!(!built.options.contains(&"c21".to_string()));
        let wire = serde_json::to_value(&built.questions[QUESTION_ID]).unwrap();
        assert_eq!(wire["criteria"]["c2"]["kind"], VOCAB.get("kind.todo"));
        assert_eq!(wire["criteria"]["c1"]["summary"], "候选1");
    }

    #[test]
    fn none_is_reserved_and_empty_candidate_list_is_refused() {
        let mut i = input(2);
        i.candidates[0].key = "none".into();
        assert!(build(&i, 0).is_err());
        assert!(build(&input(0), 0).is_err());
    }

    #[test]
    fn none_maps_to_none() {
        let built = build(&input(2), 0).unwrap();
        let mut answers = BTreeMap::new();
        answers.insert(
            QUESTION_ID.to_string(),
            Answer::Choice {
                choice: "none".into(),
                probabilities: BTreeMap::new(),
                confidence: 0.9,
            },
        );
        let resp = Response {
            model: "m".into(),
            answers,
            usage: None,
        };
        assert_eq!(
            interpret(&built, &resp, &Thresholds::default()).outcome,
            Outcome::Act(Attribution::None)
        );
    }
}
```

`decisions/urgency.rs`：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::decisions::Outcome;
    use crate::prompts::VOCAB;

    fn input() -> UrgencyInput {
        UrgencyInput {
            crew_name: "PendingCrew Mac".into(),
            waiting_on_human: vec![
                HumanRequest {
                    text: "要不要发 0.1.40".into(),
                    waited: Duration::from_secs(20 * 60),
                },
                HumanRequest {
                    text: "Polar 的 webhook 密钥放哪".into(),
                    waited: Duration::from_secs(50 * 3600),
                },
            ],
            human_mentions: 4,
            issues: vec!["额度用尽".into(), "  ".into()],
            recent: vec!["a".into()],
        }
    }

    fn score_answer(score: f64, probs: &[(&str, f64)], confidence: f64) -> Response {
        let mut answers = BTreeMap::new();
        answers.insert(
            QUESTION_ID.to_string(),
            Answer::Score {
                score,
                legend: BTreeMap::new(),
                probabilities: probs.iter().map(|(k, p)| (k.to_string(), *p)).collect(),
                confidence,
            },
        );
        Response {
            model: "m".into(),
            answers,
            usage: None,
        }
    }

    #[test]
    fn levels_are_in_order_and_quantities_are_words() {
        let built = build(&input()).unwrap();
        let q = serde_json::to_value(&built.questions[QUESTION_ID]).unwrap();
        let levels = q["criteria"].as_array().unwrap();
        assert_eq!(levels.len(), LEVELS);
        assert_eq!(levels[0], URGENCY.get("level.0"));
        assert_eq!(levels[4], URGENCY.get("level.4"));
        let crew = &built.state["crew"];
        assert_eq!(
            crew["waiting_on_human"][0]["waited"],
            VOCAB.get("wait.days"),
            "最久的排前"
        );
        assert_eq!(
            crew["waiting_on_human"][1]["waited"],
            VOCAB.get("wait.minutes")
        );
        assert_eq!(crew["human_mentions"], VOCAB.get("mentions.many"));
        assert_eq!(crew["issues"], json!(["额度用尽"]));
    }

    #[test]
    fn state_never_carries_the_durations_or_counts_as_digits() {
        let built = build(&input()).unwrap();
        let text = built.state.to_string();
        // 机组名、问题原文里本来就有的数字（0.1.40）除外，代码算的量（20 分钟、50 小时、4 次）不能出现。
        for leaked in ["1200", "180000", "\"4\"", ":4,", "50"] {
            assert!(
                !text.contains(leaked),
                "state 泄漏了原始量 {leaked}：{text}"
            );
        }
    }

    #[test]
    fn argmax_level_and_gate() {
        let built = build(&input()).unwrap();
        let th = Thresholds::default();
        let v = interpret(
            &built,
            &score_answer(2.9, &[("2", 0.1), ("3", 0.9), ("4", 0.0)], 0.8),
            &th,
        );
        assert_eq!(
            v.outcome,
            Outcome::Act(Urgency {
                score: 2.9,
                level: 3
            })
        );
        let v = interpret(
            &built,
            &score_answer(2.0, &[("0", 0.5), ("4", 0.5)], 0.0),
            &th,
        );
        assert!(matches!(
            v.outcome,
            Outcome::Defer(DeferReason::LowConfidence { .. })
        ));
    }

    #[test]
    fn out_of_range_levels_or_scores_are_malformed() {
        let built = build(&input()).unwrap();
        let th = Thresholds::default();
        for resp in [
            score_answer(5.5, &[("4", 1.0)], 1.0),
            score_answer(1.0, &[("7", 1.0)], 1.0),
            score_answer(1.0, &[("x", 1.0)], 1.0),
            score_answer(1.0, &[], 1.0),
        ] {
            assert!(matches!(
                interpret(&built, &resp, &th).outcome,
                Outcome::Defer(DeferReason::Malformed(_))
            ));
        }
    }
}
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-jev --lib decisions`
Expected: 编译失败（`WakeInput` 等未定义）。

- [ ] **Step 3: 写实现**（各自测试模块上面）

`decisions/wake.rs`：

```rust
//! 人类消息叫醒谁（spec §7.2、§17）：群里每个成员一道 noul（题号 = 成员 key，题号不发给模型）。
//! 叫醒集合 = p ≥ wake_yes 的成员；空集 → 交机长。调用方只能传**这个群的成员**（§17：只在群成员里选）。
use super::{
    Built, DecisionKind, DeferReason, MAX_RECENT, MAX_SUMMARY_CHARS, MAX_WAKE_MEMBERS, Member,
    Outcome, Thresholds, Verdict, check_keys, check_unit, clip, member_view, most_recent,
    non_empty,
};
use crate::model::{Answer, NoulCriteria, Question, Response, noul_confidence};
use crate::prompts::WAKE;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct WakeInput {
    /// 人类这条消息。
    pub message: String,
    /// 之前几条群消息，旧在前；只取最后 5 条。
    pub recent: Vec<String>,
    /// 这个群的 session 成员（不含人类）。
    pub members: Vec<Member>,
}

/// 被叫醒的成员 key，按名单（最近活跃）顺序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WakeSet {
    pub wake: Vec<String>,
}

pub fn build(input: &WakeInput) -> Result<Built, String> {
    let message = non_empty(&input.message, "消息")?;
    let members = most_recent(&input.members, MAX_WAKE_MEMBERS);
    if members.is_empty() {
        return Err("群里没有 session 成员".into());
    }
    check_keys(members.iter().map(|m| m.key.as_str()), &[])?;
    let skip = input.recent.len().saturating_sub(MAX_RECENT);
    let recent: Vec<String> = input.recent[skip..]
        .iter()
        .map(|r| clip(r, MAX_SUMMARY_CHARS))
        .collect();
    let mut questions = BTreeMap::new();
    for m in &members {
        questions.insert(
            m.key.clone(),
            Question::Noul {
                instructions: json!({
                    "member": member_view(m),
                    "question": WAKE.get("question"),
                }),
                criteria: Some(NoulCriteria {
                    yes: json!(WAKE.get("true")),
                    no: json!(WAKE.get("false")),
                }),
            },
        );
    }
    Ok(Built {
        kind: DecisionKind::Wake,
        state: json!({ "message": message, "recent": recent }),
        questions,
        options: members.iter().map(|m| m.key.clone()).collect(),
    })
}

/// 每个成员的 p（按 `built.options` 顺序）；任何一题缺失 / 类型不对 / 越界 → 整个判断作废。
pub fn probabilities(
    built: &Built,
    response: &Response,
) -> Result<Vec<(String, f64)>, DeferReason> {
    built
        .options
        .iter()
        .map(|key| match response.answers.get(key) {
            Some(Answer::Noul { noul }) => {
                check_unit(*noul)?;
                Ok((key.clone(), *noul))
            }
            Some(other) => Err(DeferReason::Malformed(format!(
                "{key} 的答案类型不对：{other:?}"
            ))),
            None => Err(DeferReason::Malformed(format!("应答里没有 {key}"))),
        })
        .collect()
}

pub fn interpret(built: &Built, response: &Response, thresholds: &Thresholds) -> Verdict<WakeSet> {
    let ps = match probabilities(built, response) {
        Ok(ps) => ps,
        Err(reason) => return Verdict::defer(reason),
    };
    let wake: Vec<String> = ps
        .iter()
        .filter(|(_, p)| *p >= thresholds.wake_yes)
        .map(|(k, _)| k.clone())
        .collect();
    // 整组的置信度 = 最没把握的那一题（|2p−1| 的最小值）。
    let confidence = ps
        .iter()
        .map(|(_, p)| noul_confidence(*p))
        .fold(1.0_f64, f64::min);
    let set = WakeSet { wake };
    let outcome = if set.wake.is_empty() {
        Outcome::Defer(DeferReason::NobodySelected)
    } else {
        Outcome::Act(set.clone())
    };
    Verdict {
        best: Some(set),
        confidence: Some(confidence),
        outcome,
    }
}
```

`decisions/attribute.rs`：

```rust
//! 随手消息归属（spec §7.2、§17）：一道 choice，选项 = 候选（最近群消息 / 未完成入口 / 工作项 / 群）+ none。
//! 候选由调用方按相关度或时间排好，只取前 20 个；发出前按种子打乱。
use super::{
    Built, DecisionKind, MAX_CANDIDATES, MAX_SUMMARY_CHARS, Thresholds, Verdict, check_keys, clip,
    gate, non_empty, read_choice,
};
use crate::model::{ChoiceCriteria, Question, Response};
use crate::prompts::{ATTRIBUTE, VOCAB};
use crate::shuffle::{seed_from, shuffle};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const QUESTION_ID: &str = "attribute";
pub const NONE: &str = "none";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    Message,
    Todo,
    WorkItem,
    Channel,
}

impl CandidateKind {
    fn word(self) -> &'static str {
        match self {
            CandidateKind::Message => VOCAB.get("kind.message"),
            CandidateKind::Todo => VOCAB.get("kind.todo"),
            CandidateKind::WorkItem => VOCAB.get("kind.work_item"),
            CandidateKind::Channel => VOCAB.get("kind.channel"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub key: String,
    pub kind: CandidateKind,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttributeInput {
    pub message: String,
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "target", content = "key", rename_all = "snake_case")]
pub enum Attribution {
    Candidate(String),
    None,
}

pub fn seed(input: &AttributeInput) -> u64 {
    seed_from(&[DecisionKind::Attribute.as_str(), &input.message])
}

pub fn build(input: &AttributeInput, seed: u64) -> Result<Built, String> {
    let message = non_empty(&input.message, "消息")?;
    let candidates: Vec<&Candidate> = input.candidates.iter().take(MAX_CANDIDATES).collect();
    if candidates.is_empty() {
        return Err("没有候选".into());
    }
    check_keys(candidates.iter().map(|c| c.key.as_str()), &[NONE])?;
    let mut options: Vec<(String, Value)> = candidates
        .iter()
        .map(|c| {
            (
                c.key.clone(),
                json!({ "kind": c.kind.word(), "summary": clip(&c.summary, MAX_SUMMARY_CHARS) }),
            )
        })
        .collect();
    options.push((NONE.into(), json!(ATTRIBUTE.get("option.none"))));
    shuffle(&mut options, seed);
    let criteria = ChoiceCriteria(options);
    let keys = criteria.keys();
    let mut questions = BTreeMap::new();
    questions.insert(
        QUESTION_ID.to_string(),
        Question::Choice {
            instructions: json!(ATTRIBUTE.get("instructions")),
            criteria,
        },
    );
    Ok(Built {
        kind: DecisionKind::Attribute,
        state: json!({ "message": message }),
        questions,
        options: keys,
    })
}

pub fn interpret(
    built: &Built,
    response: &Response,
    thresholds: &Thresholds,
) -> Verdict<Attribution> {
    match read_choice(built, response, QUESTION_ID) {
        Ok((key, confidence)) => {
            let a = if key == NONE {
                Attribution::None
            } else {
                Attribution::Candidate(key)
            };
            gate(a, confidence, thresholds.choice_min)
        }
        Err(reason) => Verdict::defer(reason),
    }
}

pub fn attribution_key(a: &Attribution) -> &str {
    match a {
        Attribution::Candidate(k) => k,
        Attribution::None => NONE,
    }
}
```

`decisions/urgency.rs`：

```rust
//! 侧边栏紧急度（spec §7.2）：每个机组一道 score，五级。级别是有序的，**不打乱**。
//! 时长、次数在这里换成词；与最近活跃合成排序键是调用方（M5b）的事。
use super::{
    Built, DecisionKind, DeferReason, MAX_RECENT, MAX_SUMMARY_CHARS, Thresholds, Verdict,
    check_unit, clip, gate, mentions_word, non_empty, wait_word,
};
use crate::model::{Answer, Question, Response};
use crate::prompts::URGENCY;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

pub const QUESTION_ID: &str = "urgency";
pub const LEVELS: usize = 5;
/// 一次最多列出的等人项。
pub const MAX_WAITING: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct HumanRequest {
    pub text: String,
    pub waited: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UrgencyInput {
    pub crew_name: String,
    pub waiting_on_human: Vec<HumanRequest>,
    /// 群里未读的 @人类 次数（代码数，给 Jev 的是词）。
    pub human_mentions: usize,
    /// 系统发现的问题，短句（额度用尽、会话起不来……）。
    pub issues: Vec<String>,
    /// 最近几条群消息，旧在前；只取最后 5 条。
    pub recent: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Urgency {
    /// 概率加权的位置，0.0–4.0。
    pub score: f64,
    /// 概率最大的那一级。
    pub level: u8,
}

pub fn build(input: &UrgencyInput) -> Result<Built, String> {
    let name = non_empty(&input.crew_name, "机组名")?;
    let mut waiting = input.waiting_on_human.clone();
    // 等得最久的排前面，列最多 8 条。
    waiting.sort_by(|a, b| b.waited.cmp(&a.waited));
    waiting.truncate(MAX_WAITING);
    let waiting: Vec<_> = waiting
        .iter()
        .map(|w| json!({ "question": clip(&w.text, MAX_SUMMARY_CHARS), "waited": wait_word(w.waited) }))
        .collect();
    let skip = input.recent.len().saturating_sub(MAX_RECENT);
    let recent: Vec<String> = input.recent[skip..]
        .iter()
        .map(|r| clip(r, MAX_SUMMARY_CHARS))
        .collect();
    let issues: Vec<String> = input
        .issues
        .iter()
        .map(|i| clip(i, MAX_SUMMARY_CHARS))
        .filter(|i| !i.is_empty())
        .collect();
    let levels = (0..LEVELS)
        .map(|i| json!(URGENCY.get(&format!("level.{i}"))))
        .collect();
    let mut questions = BTreeMap::new();
    questions.insert(
        QUESTION_ID.to_string(),
        Question::Score {
            instructions: json!(URGENCY.get("instructions")),
            criteria: levels,
        },
    );
    Ok(Built {
        kind: DecisionKind::Urgency,
        state: json!({ "crew": {
            "name": name,
            "waiting_on_human": waiting,
            "human_mentions": mentions_word(input.human_mentions),
            "issues": issues,
            "recent": recent,
        }}),
        questions,
        options: (0..LEVELS).map(|i| i.to_string()).collect(),
    })
}

pub fn interpret(_built: &Built, response: &Response, thresholds: &Thresholds) -> Verdict<Urgency> {
    let (score, probabilities, confidence) = match response.answers.get(QUESTION_ID) {
        Some(Answer::Score {
            score,
            probabilities,
            confidence,
            ..
        }) => (*score, probabilities, *confidence),
        Some(other) => {
            return Verdict::defer(DeferReason::Malformed(format!(
                "urgency 的答案类型不对：{other:?}"
            )));
        }
        None => return Verdict::defer(DeferReason::Malformed("应答里没有 urgency".into())),
    };
    if let Err(reason) = check_unit(confidence) {
        return Verdict::defer(reason);
    }
    if !(0.0..=(LEVELS - 1) as f64).contains(&score) {
        return Verdict::defer(DeferReason::Malformed(format!("score 越界 {score}")));
    }
    let mut level: Option<(u8, f64)> = None;
    for (key, p) in probabilities {
        let Ok(l) = key.parse::<u8>() else {
            return Verdict::defer(DeferReason::Malformed(format!("级别名不是数字 {key:?}")));
        };
        if usize::from(l) >= LEVELS {
            return Verdict::defer(DeferReason::Malformed(format!("级别越界 {l}")));
        }
        if level.is_none_or(|(_, best)| *p > best) {
            level = Some((l, *p));
        }
    }
    let Some((level, _)) = level else {
        return Verdict::defer(DeferReason::Malformed("score 没有 probabilities".into()));
    };
    gate(Urgency { score, level }, confidence, thresholds.score_min)
}
```

- [ ] **Step 4: 跑通**

Run: `cargo test -p crewd-jev --lib`
Expected: `52 passed; 1 ignored`（+ wake 5、attribute 3、urgency 4）。

- [ ] **Step 5: 变异核对**

1. `wake::interpret` 的 `*p >= thresholds.wake_yes` 改成 `>` → Expected `wake_set_is_everyone_at_or_above_the_line` 红（写计划时核过）。
2. `wake::probabilities` 里 `None => Err(...)` 改成跳过缺失成员（`filter_map`）→ Expected `a_missing_member_answer_voids_the_whole_judgement` 红。
3. `urgency::build` 里 `wait_word(w.waited)` 换成 `w.waited.as_secs().to_string()` → Expected `levels_are_in_order_and_quantities_are_words` 与 `state_never_carries_the_durations_or_counts_as_digits` 红。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/jev/src/decisions/mod.rs crates/jev/src/decisions/wake.rs crates/jev/src/decisions/attribute.rs crates/jev/src/decisions/urgency.rs
git diff --cached --stat
git commit -m "crewd-jev: wake set (per-member noul), casual-message attribution, sidebar urgency score

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: 诊断日志 + 门面（`diag`、`jev`）

`Jev` 把前面所有部件串起来：构造（失败 → `InvalidInput`，不联网）→ 未配置（→ `Unconfigured`，不联网）→ 脱敏（日志摘要取自脱敏后的 state）→ `Client::evaluate`（里面再脱一遍，幂等）→ 判定 → 记一行日志。日志写失败只计数。

**Files:**
- Create: `crewd/crates/jev/src/diag.rs`、`crewd/crates/jev/src/jev.rs`、`crewd/crates/jev/tests/jev.rs`
- Modify: `crewd/crates/jev/src/lib.rs`

**Interfaces:**
- Consumes: Task 3 的 `client::*`、Task 5 / 6 的全部决策、`scrub::scrub_request`、`model::{DEFAULT_BASE_URL, DEFAULT_MODEL}`。
- Produces:
  - `crewd_jev::diag`：常量 `DEFAULT_MAX_BYTES`（5 MiB）、`EXCERPT_CHARS`（160）；`struct DiagRecord { at_unix_ms, decision, model, state_chars, state_excerpt, options, scrub_hits, attempts, latency_ms, outcome, reason, confidence, answer }`；`struct DiagLog`（`fn open(path: impl Into<PathBuf>) -> io::Result<DiagLog>`、`fn with_max_bytes(self, u64) -> DiagLog`、`fn path(&self) -> &Path`、`fn rotated_path(&self) -> PathBuf`、`fn append(&self, &DiagRecord) -> io::Result<()>`）
  - `crewd_jev::jev`：`struct JevConfig { enabled, base_url, model, thresholds }`（`Default`、`Serialize + Deserialize`）；`struct Jev`（`fn new(&JevConfig, Option<ApiKey>, Option<DiagLog>) -> Result<Jev, String>`、`fn with_client(Option<Client>, &JevConfig, Option<DiagLog>) -> Jev`、`fn is_configured(&self) -> bool`、`fn diag_failures(&self) -> u64`、`async fn route_todo / wake / attribute / urgency`）

- [ ] **Step 1: 写失败的测试**

`lib.rs`：

```rust
pub mod client;
pub mod decisions;
pub mod diag;
pub mod error;
pub mod jev;
pub mod key;
pub mod model;
pub mod prompts;
pub mod scrub;
pub mod shuffle;
```

`crates/jev/src/diag.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(n: usize) -> DiagRecord {
        DiagRecord {
            at_unix_ms: 1,
            decision: "route_todo",
            model: "jev-1.13.0".into(),
            state_chars: n,
            state_excerpt: "{\"todo\":\"修滚动\"}".into(),
            options: vec!["captain".into(), "m1".into()],
            scrub_hits: 0,
            attempts: 1,
            latency_ms: 900,
            outcome: "act",
            reason: None,
            confidence: Some(0.9),
            answer: json!({"target": "member", "key": "m1"}),
        }
    }

    #[test]
    fn one_json_object_per_line_in_a_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = DiagLog::open(dir.path().join("jev-diagnostics.jsonl")).unwrap();
        log.append(&record(1)).unwrap();
        log.append(&record(2)).unwrap();
        let text = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["state_chars"], 2);
        assert_eq!(lines[0]["answer"]["key"], "m1");
        let mode = std::fs::metadata(log.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn existing_loose_file_is_tightened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.jsonl");
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _log = DiagLog::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn rotates_into_a_single_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = DiagLog::open(dir.path().join("d.jsonl"))
            .unwrap()
            .with_max_bytes(600);
        for i in 0..10 {
            log.append(&record(i)).unwrap();
        }
        let current = std::fs::read_to_string(log.path()).unwrap();
        let old = std::fs::read_to_string(log.rotated_path()).unwrap();
        assert!(current.len() <= 600, "当前文件 {} 字节", current.len());
        assert!(!old.is_empty());
        let last: Value = serde_json::from_str(current.lines().last().unwrap()).unwrap();
        assert_eq!(last["state_chars"], 9, "最新一条在当前文件里");
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 2, "只留一份旧文件");
    }
}
```

`crates/jev/src/jev.rs`：先放一行占位让模块存在（下一步整份替换）：

```rust
//! 门面（Task 7 Step 3 写入）。
```

`crates/jev/tests/jev.rs`：

```rust
//! 门面对着假 Jev：未配置不联网、端到端判定、诊断日志（不含 key、摘要已脱敏）。
mod support;

use crewd_jev::client::{ApiKey, Client, ClientConfig, RetryPolicy};
use crewd_jev::decisions::route::{RouteTarget, RouteTodoInput};
use crewd_jev::decisions::wake::{WakeInput, WakeSet};
use crewd_jev::decisions::{DeferReason, Member, Outcome};
use crewd_jev::diag::DiagLog;
use crewd_jev::error::CallError;
use crewd_jev::jev::{Jev, JevConfig};
use serde_json::Value;
use std::time::Duration;
use support::{FakeJev, Reply, choice_body, noul_body};

const TEST_KEY: &str = "test-key-not-real-0123456789";

fn member(key: &str, rank: usize) -> Member {
    Member {
        key: key.into(),
        name: format!("成员{key}"),
        role: format!("负责 {key}"),
        work: vec![],
        busy: false,
        recency_rank: rank,
    }
}

fn route_input(todo: &str) -> RouteTodoInput {
    RouteTodoInput {
        todo: todo.into(),
        members: vec![member("scroll", 1), member("release", 2)],
    }
}

fn jev_against(base: &str, diag: Option<DiagLog>) -> Jev {
    let mut cfg = ClientConfig::new(base, ApiKey::new(TEST_KEY).unwrap());
    cfg.retry = RetryPolicy {
        max_attempts: 2,
        base_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(20),
    };
    cfg.attempt_timeout = Duration::from_millis(500);
    cfg.deadline = Duration::from_secs(2);
    Jev::with_client(Some(Client::new(cfg).unwrap()), &JevConfig::default(), diag)
}

fn diag_lines(log: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
async fn without_a_key_nothing_leaves_and_everything_defers() {
    let fake = FakeJev::start(vec![]).await;
    let mut config = JevConfig::default();
    config.base_url = fake.base_url.clone();
    let dir = tempfile::tempdir().unwrap();
    let diag = DiagLog::open(dir.path().join("jev.jsonl")).unwrap();
    let jev = Jev::new(&config, None, Some(diag)).unwrap();
    assert!(!jev.is_configured());
    let v = jev.route_todo(&route_input("修滚动")).await;
    assert_eq!(v.outcome, Outcome::Defer(DeferReason::Unconfigured));
    config.enabled = false;
    let off = Jev::new(&config, ApiKey::new(TEST_KEY), None).unwrap();
    assert!(!off.is_configured(), "关掉了，有 key 也不发");
    assert_eq!(fake.requests().len(), 0);
    let lines = diag_lines(&dir.path().join("jev.jsonl"));
    assert_eq!(lines[0]["outcome"], "defer");
    assert_eq!(lines[0]["reason"], "unconfigured");
}

#[test]
fn bad_base_url_is_reported_not_silently_unconfigured() {
    let mut config = JevConfig::default();
    config.base_url = "http://api.typesafe.ai".into();
    assert!(Jev::new(&config, ApiKey::new(TEST_KEY), None).is_err());
}

#[tokio::test]
async fn route_acts_on_a_confident_answer_and_logs_without_the_key() {
    let fake = FakeJev::start(vec![Reply::ok(choice_body("route", "scroll", 0.9))]).await;
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("jev.jsonl");
    let jev = jev_against(&fake.base_url, Some(DiagLog::open(&log).unwrap()));
    let v = jev.route_todo(&route_input("终端滚动跳动")).await;
    assert_eq!(
        v.outcome,
        Outcome::Act(RouteTarget::Member("scroll".into()))
    );
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(!text.contains(TEST_KEY), "诊断日志里出现了 key");
    assert!(!text.to_ascii_lowercase().contains("bearer"));
    let line = &diag_lines(&log)[0];
    assert_eq!(line["decision"], "route_todo");
    assert_eq!(line["outcome"], "act");
    assert_eq!(line["confidence"], 0.9);
    assert_eq!(line["answer"]["key"], "scroll");
    assert_eq!(line["options"].as_array().unwrap().len(), 5);
    assert!(
        line["state_excerpt"]
            .as_str()
            .unwrap()
            .contains("终端滚动跳动")
    );
    assert_eq!(jev.diag_failures(), 0);
}

#[tokio::test]
async fn secrets_are_scrubbed_from_the_request_and_the_log() {
    let secret = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
    let fake = FakeJev::start(vec![Reply::ok(choice_body("route", "release", 0.9))]).await;
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("jev.jsonl");
    let jev = jev_against(&fake.base_url, Some(DiagLog::open(&log).unwrap()));
    jev.route_todo(&route_input(&format!("CI 用的 token 换成 {secret}")))
        .await;
    assert!(!fake.requests()[0].body_text().contains(&secret));
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(!text.contains(&secret));
    assert_eq!(diag_lines(&log)[0]["scrub_hits"], 1);
}

#[tokio::test]
async fn low_confidence_and_errors_defer_to_the_captain() {
    let fake = FakeJev::start(vec![
        Reply::ok(choice_body("route", "captain", 0.2)),
        Reply::status(401),
    ])
    .await;
    let jev = jev_against(&fake.base_url, None);
    let v = jev.route_todo(&route_input("排一下优先级")).await;
    assert_eq!(v.best, Some(RouteTarget::Captain));
    assert_eq!(
        v.outcome,
        Outcome::Defer(DeferReason::LowConfidence { confidence: 0.2 })
    );
    let v = jev.route_todo(&route_input("再排一下")).await;
    assert_eq!(
        v.outcome,
        Outcome::Defer(DeferReason::Call(CallError::Unauthorized { status: 401 }))
    );
}

#[tokio::test]
async fn invalid_input_never_reaches_the_network() {
    let fake = FakeJev::start(vec![]).await;
    let jev = jev_against(&fake.base_url, None);
    let v = jev.route_todo(&route_input("   ")).await;
    assert!(matches!(
        v.outcome,
        Outcome::Defer(DeferReason::InvalidInput(_))
    ));
    assert_eq!(fake.requests().len(), 0);
}

#[tokio::test]
async fn wake_end_to_end() {
    let fake = FakeJev::start(vec![Reply::ok(noul_body(&[
        ("scroll", 0.92),
        ("release", 0.08),
    ]))])
    .await;
    let jev = jev_against(&fake.base_url, None);
    let v = jev
        .wake(&WakeInput {
            message: "试了，还是会跳".into(),
            recent: vec!["滚动修复：改好了你试试".into()],
            members: vec![member("scroll", 1), member("release", 2)],
        })
        .await;
    assert_eq!(
        v.outcome,
        Outcome::Act(WakeSet {
            wake: vec!["scroll".into()]
        })
    );
    let body: Value = serde_json::from_slice(&fake.requests()[0].body).unwrap();
    assert_eq!(body["questions"]["scroll"]["type"], "noul");
    assert_eq!(body["state"]["message"], "试了，还是会跳");
}
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-jev --lib diag` 与 `cargo test -p crewd-jev --test jev`
Expected: 都编译失败（`DiagRecord`、`Jev` 未定义）。

- [ ] **Step 3: 写实现**

`diag.rs`（测试模块上面）：

```rust
//! 本机诊断日志（spec §7.1）：每次判断一行 JSON，记输入摘要（脱敏后）、答案、置信度、耗时。
//! 不记 key，不上传。文件 0600；超过上限就把旧文件挪成 `<名字>.1`（只留一份旧的）。
use serde::Serialize;
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const EXCERPT_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiagRecord {
    pub at_unix_ms: u64,
    pub decision: &'static str,
    pub model: String,
    /// 脱敏后 state 的字符数与开头一段。
    pub state_chars: usize,
    pub state_excerpt: String,
    /// 按发出顺序的选项名 / 题号。
    pub options: Vec<String>,
    pub scrub_hits: usize,
    pub attempts: u32,
    pub latency_ms: u64,
    /// "act" | "defer"
    pub outcome: &'static str,
    pub reason: Option<String>,
    pub confidence: Option<f64>,
    pub answer: Value,
}

#[derive(Debug)]
pub struct DiagLog {
    path: PathBuf,
    max_bytes: u64,
    file: Mutex<Option<File>>,
}

fn open_append(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    // 早先以更宽权限建过的文件，收紧回 0600。
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

impl DiagLog {
    /// 父目录必须已存在（crewd 的数据目录，0700）。
    pub fn open(path: impl Into<PathBuf>) -> io::Result<DiagLog> {
        let path = path.into();
        let file = open_append(&path)?;
        Ok(DiagLog {
            path,
            max_bytes: DEFAULT_MAX_BYTES,
            file: Mutex::new(Some(file)),
        })
    }

    pub fn with_max_bytes(mut self, max_bytes: u64) -> DiagLog {
        self.max_bytes = max_bytes;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn rotated_path(&self) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".1");
        self.path.with_file_name(name)
    }

    pub fn append(&self, record: &DiagRecord) -> io::Result<()> {
        let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let len = match guard.as_ref() {
            Some(f) => f.metadata()?.len(),
            None => 0,
        };
        if guard.is_none() || (len > 0 && len + line.len() as u64 > self.max_bytes) {
            *guard = None;
            if len > 0 {
                std::fs::rename(&self.path, self.rotated_path())?;
            }
            *guard = Some(open_append(&self.path)?);
        }
        let file = guard.as_mut().expect("刚打开");
        file.write_all(&line)
    }
}
```

`jev.rs`（整个文件）：

```rust
//! 对外的门面：给决策输入，拿 `Verdict`。构造 → 脱敏 → 调用 → 判定 → 记诊断日志，一条龙。
//! 没开 / 没 key：不碰网络，直接 `Defer(Unconfigured)`（spec §7.1：未配置 = 低置信度）。
//! 诊断日志写失败不影响判断，只计数（`diag_failures`）。
use crate::client::{ApiKey, Client, ClientConfig};
use crate::decisions::attribute::{self, AttributeInput, Attribution};
use crate::decisions::route::{self, RouteTarget, RouteTodoInput};
use crate::decisions::urgency::{self, Urgency, UrgencyInput};
use crate::decisions::wake::{self, WakeInput, WakeSet};
use crate::decisions::{Built, DecisionKind, DeferReason, Outcome, Thresholds, Verdict, clip};
use crate::diag::{DiagLog, DiagRecord, EXCERPT_CHARS};
use crate::model::{DEFAULT_BASE_URL, DEFAULT_MODEL, Response};
use crate::scrub::scrub_request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 设置页的 Jev 一节（spec §7.4）。key 不在这里：它在 `KeyStore`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevConfig {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub thresholds: Thresholds,
}

impl Default for JevConfig {
    fn default() -> Self {
        JevConfig {
            enabled: true,
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
            thresholds: Thresholds::default(),
        }
    }
}

type Interpret<T> = fn(&Built, &Response, &Thresholds) -> Verdict<T>;

#[derive(Debug)]
pub struct Jev {
    client: Option<Client>,
    model: String,
    thresholds: Thresholds,
    diag: Option<DiagLog>,
    diag_failures: AtomicU64,
}

impl Jev {
    /// 关着或没有 key → 未配置（不报错）；base URL 不合法 → Err（设置页要能提示）。
    pub fn new(
        config: &JevConfig,
        key: Option<ApiKey>,
        diag: Option<DiagLog>,
    ) -> Result<Jev, String> {
        let client = match (config.enabled, key) {
            (true, Some(key)) => Some(Client::new(ClientConfig::new(
                config.base_url.clone(),
                key,
            ))?),
            _ => None,
        };
        Ok(Jev::with_client(client, config, diag))
    }

    /// 测试与评测用：自带调好超时的 `Client`。
    pub fn with_client(client: Option<Client>, config: &JevConfig, diag: Option<DiagLog>) -> Jev {
        Jev {
            client,
            model: config.model.clone(),
            thresholds: config.thresholds,
            diag,
            diag_failures: AtomicU64::new(0),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.client.is_some()
    }

    pub fn diag_failures(&self) -> u64 {
        self.diag_failures.load(Ordering::Relaxed)
    }

    pub async fn route_todo(&self, input: &RouteTodoInput) -> Verdict<RouteTarget> {
        let built = route::build(input, route::seed(input));
        self.decide(DecisionKind::RouteTodo, built, route::interpret)
            .await
    }

    pub async fn wake(&self, input: &WakeInput) -> Verdict<WakeSet> {
        self.decide(DecisionKind::Wake, wake::build(input), wake::interpret)
            .await
    }

    pub async fn attribute(&self, input: &AttributeInput) -> Verdict<Attribution> {
        let built = attribute::build(input, attribute::seed(input));
        self.decide(DecisionKind::Attribute, built, attribute::interpret)
            .await
    }

    pub async fn urgency(&self, input: &UrgencyInput) -> Verdict<Urgency> {
        self.decide(
            DecisionKind::Urgency,
            urgency::build(input),
            urgency::interpret,
        )
        .await
    }

    async fn decide<T: Clone + Serialize>(
        &self,
        kind: DecisionKind,
        built: Result<Built, String>,
        interpret: Interpret<T>,
    ) -> Verdict<T> {
        let built = match built {
            Ok(b) => b,
            Err(e) => {
                let v = Verdict::defer(DeferReason::InvalidInput(e));
                self.log(kind, None, 0, 0, Duration::ZERO, &v);
                return v;
            }
        };
        let Some(client) = &self.client else {
            let v = Verdict::defer(DeferReason::Unconfigured);
            self.log(kind, None, 0, 0, Duration::ZERO, &v);
            return v;
        };
        let mut request = built.request(&self.model);
        // 先在这里脱敏：诊断日志的摘要取自脱敏后的 state。客户端还会再脱一遍（幂等，命中 0）。
        let scrub_hits = scrub_request(&mut request);
        let (result, report) = client.evaluate(&request).await;
        let verdict = match result {
            Ok(response) => interpret(&built, &response, &self.thresholds),
            Err(e) => Verdict::defer(DeferReason::Call(e)),
        };
        let excerpt = Some((&built, &request.state));
        self.log(
            kind,
            excerpt,
            scrub_hits,
            report.attempts,
            report.latency,
            &verdict,
        );
        verdict
    }

    fn log<T: Serialize>(
        &self,
        kind: DecisionKind,
        sent: Option<(&Built, &Value)>,
        scrub_hits: usize,
        attempts: u32,
        latency: Duration,
        verdict: &Verdict<T>,
    ) {
        let Some(diag) = &self.diag else { return };
        let state_text = sent.map(|(_, s)| s.to_string()).unwrap_or_default();
        let (outcome, reason) = match &verdict.outcome {
            Outcome::Act(_) => ("act", None),
            Outcome::Defer(r) => ("defer", Some(r.label())),
        };
        let record = DiagRecord {
            at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            decision: kind.as_str(),
            model: self.model.clone(),
            state_chars: state_text.chars().count(),
            state_excerpt: clip(&state_text, EXCERPT_CHARS),
            options: sent.map(|(b, _)| b.options.clone()).unwrap_or_default(),
            scrub_hits,
            attempts,
            latency_ms: latency.as_millis() as u64,
            outcome,
            reason,
            confidence: verdict.confidence,
            answer: serde_json::to_value(&verdict.best).unwrap_or(Value::Null),
        };
        if diag.append(&record).is_err() {
            self.diag_failures.fetch_add(1, Ordering::Relaxed);
        }
    }
}
```

- [ ] **Step 4: 跑通**

Run: `cargo test -p crewd-jev --lib`，再 `cargo test -p crewd-jev --test jev`
Expected: 单测 `55 passed; 1 ignored`（+ diag 3）；集成测试 `7 passed`。

- [ ] **Step 5: 变异核对**

1. `Jev::decide` 里删掉 `let scrub_hits = scrub_request(&mut request);`（换成 `let scrub_hits = 0;`）→ Expected `secrets_are_scrubbed_from_the_request_and_the_log` 红（日志摘要里出现明文；请求本身仍由客户端脱敏，所以红的是日志那条断言——这正是这一步在防的东西）。
2. `Jev::new` 里把 `(true, Some(key))` 改成 `(_, Some(key))` → Expected `without_a_key_nothing_leaves_and_everything_defers` 红（关掉了还发）。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/jev/src/diag.rs crates/jev/src/jev.rs crates/jev/src/lib.rs crates/jev/tests/jev.rs
git diff --cached --stat
git commit -m "crewd-jev: Jev facade (unconfigured never calls out) and local JSONL diagnostics without the key

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: 中文实测集 + `jev-eval`（`eval`）

40 条合成中文用例（三个虚构机组：PendingCrew Mac 开发、大绿豆后端、个人博客；派活 13、叫醒 11、归属 8、紧急度 8），只用合成数据。`eval` 是纯计算（数据集、判对错、分桶、阈值推荐、Markdown），有单测；`jev-eval` 只做 IO（读 key、发请求、录原文、写报告），没有单测，用一次「打到没人监听的本机端口」的干跑验证接线，不花钱。

**Files:**
- Create: `crewd/crates/jev/eval/cases.zh.json`、`crewd/crates/jev/src/eval.rs`、`crewd/crates/jev/src/bin/jev-eval.rs`
- Modify: `crewd/crates/jev/src/lib.rs`、`crewd/README.md`

**Interfaces:**
- Consumes: 全部决策模块、`client::{ApiKey, Client, ClientConfig}`、`key::{KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT}`、`shuffle::seed_from`、`model::{DEFAULT_BASE_URL, DEFAULT_MODEL, noul_confidence}`。
- Produces（`crewd_jev::eval`）：`struct Dataset { crews, cases }`（`fn parse(&str) -> Result<Dataset, String>`、`fn build_case(&self, &Case, seed: u64) -> Result<Built, String>`、`fn validate(&self) -> Result<(), String>`、`fn case(&self, id) -> Option<&Case>`）；`struct CrewFixture`、`struct WaitingFixture`；`enum Case { RouteTodo {..}, Wake {..}, Attribute {..}, Urgency {..} }`（`fn id`、`fn kind`）；`struct Judged { id, kind, seed, answer, confidence, correct, picked_first, options, nouls, error }`（`fn failed(&Case, seed, String) -> Judged`）；`fn judge(&Case, seed, &Built, &Response) -> Judged`；`const BUCKETS`；`struct BucketStat`、`fn calibration`；`struct Pick { threshold, precision, coverage, acted }`、`fn recommend_threshold(&[&Judged], target: f64) -> Option<Pick>`；`struct WakePick`、`fn recommend_wake`；`struct KindSummary`、`struct Summary`（`fn markdown(&self) -> String`）、`fn summarize(&[Judged], &[Duration], target) -> Summary`、`fn percentile_ms`。二进制 `jev-eval`（参数见文件头注释）。

- [ ] **Step 1: 写数据集**（交付物本身，逐字照抄）

`crates/jev/eval/cases.zh.json`：

```json
{
  "crews": {
    "mac": {
      "name": "PendingCrew Mac 开发",
      "members": [
        { "key": "captain", "name": "机长", "role": "机长：规划、拆分、验收整个 Mac 版的工作", "work": ["本周发版计划"], "busy": false, "recency_rank": 2 },
        { "key": "scroll", "name": "滚动修复", "role": "修终端视图的滚动、重绘和选区问题", "work": ["终端滚动跳动", "选区高亮残影"], "busy": true, "recency_rank": 1 },
        { "key": "release", "name": "发版", "role": "负责 Mac 版打包、签名、公证、自动更新源和 Homebrew tap", "work": ["新版本 dmg 与公证", "自动更新源更新"], "busy": false, "recency_rank": 4 },
        { "key": "crewd", "name": "crewd 后端", "role": "用 Rust 写新的后台进程 crewd：会话驱动、存储、接口", "work": ["claude 会话投递状态机", "SQLite 存储迁移"], "busy": true, "recency_rank": 3 },
        { "key": "ios", "name": "iOS 配对", "role": "iOS 客户端与 Mac 的配对和加密直连", "work": ["证书指纹配对"], "busy": false, "recency_rank": 6 },
        { "key": "docs", "name": "文档", "role": "维护设计文档、实现计划和交付记录", "work": ["设计文档交付记录回写"], "busy": false, "recency_rank": 8 }
      ]
    },
    "edge": {
      "name": "大绿豆后端",
      "members": [
        { "key": "captain", "name": "机长", "role": "机长：大绿豆后端的规划与验收", "work": [], "busy": false, "recency_rank": 3 },
        { "key": "billing", "name": "计费", "role": "钱包与计费：网页支付、应用内购买、钱包余额门禁", "work": ["钱包门禁缓存", "支付回调对账"], "busy": true, "recency_rank": 1 },
        { "key": "voice", "name": "群语音", "role": "群语音：媒体容器与实时音频转发", "work": ["语音断续修复"], "busy": false, "recency_rank": 5 },
        { "key": "migrate", "name": "数据库迁移", "role": "数据库表结构、迁移脚本与表权限", "work": ["bot 可见性迁移", "表权限授权修复"], "busy": false, "recency_rank": 2 },
        { "key": "board", "name": "看板", "role": "运维看板：告警、埋点、错误上报", "work": ["告警面板"], "busy": true, "recency_rank": 4 }
      ]
    },
    "solo": {
      "name": "个人博客",
      "members": [
        { "key": "captain", "name": "机长", "role": "机长：个人博客站点", "work": [], "busy": false, "recency_rank": 2 },
        { "key": "blog", "name": "博客写作", "role": "写博客文章、维护静态站点", "work": ["Rust 异步入门文章"], "busy": false, "recency_rank": 1 }
      ]
    }
  },
  "cases": [
    { "decision": "route_todo", "id": "route-01", "crew": "mac", "todo": "终端里滚动到底部的时候画面会上下跳一下，帮忙看看", "expect": ["scroll"] },
    { "decision": "route_todo", "id": "route-02", "crew": "mac", "todo": "新版本的公证一直卡在 In Progress，查一下怎么回事", "expect": ["release"] },
    { "decision": "route_todo", "id": "route-03", "crew": "mac", "todo": "crewd 的 SQLite 迁移跑完以后 sessions 表少了几列", "expect": ["crewd"] },
    { "decision": "route_todo", "id": "route-04", "crew": "mac", "todo": "iPhone 上扫码配对总是提示证书不匹配", "expect": ["ios"] },
    { "decision": "route_todo", "id": "route-05", "crew": "mac", "todo": "把上周定下来的工作项设计回写到设计文档里，交付记录也补上", "expect": ["docs"] },
    { "decision": "route_todo", "id": "route-06", "crew": "mac", "todo": "Mac 版加一个快捷键面板，把所有快捷键列出来", "expect": ["new_session", "captain"] },
    { "decision": "route_todo", "id": "route-07", "crew": "mac", "todo": "这周先发版还是先把 crewd 做完？你们排一下优先级", "expect": ["captain"] },
    { "decision": "route_todo", "id": "route-08", "crew": "mac", "todo": "我想从零开始做一个安卓版的客户端，单独一个仓库", "expect": ["new_crew"] },
    { "decision": "route_todo", "id": "route-09", "crew": "edge", "todo": "支付回调重复触发，有用户的钱包被扣了两次", "expect": ["billing"] },
    { "decision": "route_todo", "id": "route-10", "crew": "edge", "todo": "群语音里有人说话断断续续，人一多就更明显", "expect": ["voice"] },
    { "decision": "route_todo", "id": "route-11", "crew": "edge", "todo": "告警面板上服务端错误的告警一直在响，看看是不是阈值设错了", "expect": ["board"] },
    { "decision": "route_todo", "id": "route-12", "crew": "edge", "todo": "上线前把计费和数据库迁移一起过一遍，谁先谁后你们定好再动手", "expect": ["captain"] },
    { "decision": "route_todo", "id": "route-13", "crew": "solo", "todo": "写一篇介绍 tokio select 宏的文章", "expect": ["blog"] },

    { "decision": "wake", "id": "wake-01", "crew": "mac", "channel": ["captain", "scroll", "release", "crewd"], "recent": ["滚动修复：选区残影已经修好了，等合并"], "message": "@发版 新版本的 dmg 打好了吗？", "expect": ["release"] },
    { "decision": "wake", "id": "wake-02", "crew": "mac", "channel": ["captain", "scroll", "crewd"], "recent": ["滚动修复：我把滚动锚点改成按行对齐了，你再试试"], "message": "试了，还是会跳，而且选区也花了", "expect": ["scroll"] },
    { "decision": "wake", "id": "wake-03", "crew": "mac", "channel": ["captain", "scroll", "release", "crewd", "docs"], "recent": ["发版：新版本已经发出去了"], "message": "谢谢大家，今天辛苦了", "expect": [] },
    { "decision": "wake", "id": "wake-04", "crew": "mac", "channel": ["captain", "scroll", "release", "crewd", "ios", "docs"], "recent": [], "message": "crewd 的接口我想改成 gRPC，先别往下写了", "expect": ["crewd"], "optional": ["captain"] },
    { "decision": "wake", "id": "wake-05", "crew": "mac", "channel": ["captain", "ios", "crewd"], "recent": ["iOS 配对：需要你确认一下，配对码是显示在 Mac 上还是 iPhone 上？"], "message": "显示在 Mac 上", "expect": ["ios"] },
    { "decision": "wake", "id": "wake-06", "crew": "edge", "channel": ["captain", "billing", "migrate"], "recent": ["数据库迁移：bot 可见性的迁移准备好了，今晚推"], "message": "迁移今晚先别推，计费那边的表还在改", "expect": ["migrate"], "optional": ["billing", "captain"] },
    { "decision": "wake", "id": "wake-07", "crew": "edge", "channel": ["captain", "billing", "voice", "migrate", "board"], "recent": ["看板：错误告警突然多了，我在查"], "message": "那个告警是我手动触发的测试，别查了", "expect": ["board"], "optional": ["captain"] },
    { "decision": "wake", "id": "wake-08", "crew": "edge", "channel": ["captain", "billing", "voice"], "recent": [], "message": "今天中午吃什么好", "expect": [] },
    { "decision": "wake", "id": "wake-09", "crew": "solo", "channel": ["captain", "blog"], "recent": ["博客写作：tokio select 那篇初稿写好了"], "message": "标题改成《tokio select 的几个坑》", "expect": ["blog"] },
    { "decision": "wake", "id": "wake-10", "crew": "mac", "channel": ["captain", "scroll", "release"], "recent": [], "message": "机长，这周的发版计划发我看一下", "expect": ["captain"], "optional": ["release"] },
    { "decision": "wake", "id": "wake-11", "crew": "edge", "channel": ["captain", "billing", "voice", "migrate", "board"], "recent": [], "message": "@计费 @群语音 你们俩对一下，语音通话要不要按时长扣费", "expect": ["billing", "voice"], "optional": ["captain"] },

    { "decision": "attribute", "id": "attr-01", "message": "还是不行，换个思路吧", "candidates": [
      { "key": "m-301", "kind": "message", "summary": "滚动修复：我把滚动锚点改成按行对齐了，你再试试" },
      { "key": "m-298", "kind": "message", "summary": "发版：dmg 已经传到发布页了" },
      { "key": "todo-41", "kind": "todo", "summary": "iPhone 扫码配对提示证书不匹配" } ], "expect": ["m-301"] },
    { "decision": "attribute", "id": "attr-02", "message": "dmg 我下好了，装上能打开", "candidates": [
      { "key": "m-301", "kind": "message", "summary": "滚动修复：我把滚动锚点改成按行对齐了，你再试试" },
      { "key": "m-298", "kind": "message", "summary": "发版：dmg 已经传到发布页了" },
      { "key": "todo-40", "kind": "todo", "summary": "自动更新没有弹出新版本提示" } ], "expect": ["m-298"] },
    { "decision": "attribute", "id": "attr-03", "message": "早上好", "candidates": [
      { "key": "m-301", "kind": "message", "summary": "滚动修复：我把滚动锚点改成按行对齐了，你再试试" },
      { "key": "m-298", "kind": "message", "summary": "发版：dmg 已经传到发布页了" },
      { "key": "todo-41", "kind": "todo", "summary": "iPhone 扫码配对提示证书不匹配" } ], "expect": ["none"] },
    { "decision": "attribute", "id": "attr-04", "message": "配对那个，证书指纹我看对上了", "candidates": [
      { "key": "todo-41", "kind": "todo", "summary": "iPhone 扫码配对提示证书不匹配" },
      { "key": "w7", "kind": "work_item", "summary": "证书指纹配对：配对时交换自签证书指纹" },
      { "key": "m-298", "kind": "message", "summary": "发版：dmg 已经传到发布页了" } ], "expect": ["todo-41", "w7"] },
    { "decision": "attribute", "id": "attr-05", "message": "扣两次费的那个用户我已经手动退款了", "candidates": [
      { "key": "todo-12", "kind": "todo", "summary": "支付回调重复触发，有用户的钱包被扣了两次" },
      { "key": "w3", "kind": "work_item", "summary": "钱包门禁缓存" },
      { "key": "m-77", "kind": "message", "summary": "群语音：断续问题定位到容器的抖动缓冲" } ], "expect": ["todo-12"] },
    { "decision": "attribute", "id": "attr-06", "message": "新版本的公证又失败了，报错说签名没带时间戳", "candidates": [
      { "key": "ch-release", "kind": "channel", "summary": "发版群：打包、签名、公证、自动更新" },
      { "key": "ch-scroll", "kind": "channel", "summary": "滚动问题排查群" },
      { "key": "ch-voice", "kind": "channel", "summary": "群语音排查群" } ], "expect": ["ch-release"] },
    { "decision": "attribute", "id": "attr-07", "message": "三个人通话的时候第二个人的声音会断", "candidates": [
      { "key": "ch-release", "kind": "channel", "summary": "发版群：打包、签名、公证、自动更新" },
      { "key": "ch-scroll", "kind": "channel", "summary": "滚动问题排查群" },
      { "key": "ch-voice", "kind": "channel", "summary": "群语音排查群" } ], "expect": ["ch-voice"] },
    { "decision": "attribute", "id": "attr-08", "message": "这篇你觉得怎么样，结尾是不是太突然了", "candidates": [
      { "key": "m-12", "kind": "message", "summary": "博客写作：tokio select 那篇初稿写好了，链接在这" },
      { "key": "todo-3", "kind": "todo", "summary": "给博客加订阅源" } ], "expect": ["m-12"] },

    { "decision": "urgency", "id": "urg-01", "crew_name": "个人博客", "recent": ["博客写作：初稿写好了"], "expect_level": [0, 1] },
    { "decision": "urgency", "id": "urg-02", "crew_name": "PendingCrew Mac 开发", "waiting": [{ "question": "要不要把滚动修复合进这次发版", "waited_minutes": 20 }], "human_mentions": 1, "recent": ["crewd 后端：投递状态机测试全绿"], "expect_level": [2, 2] },
    { "decision": "urgency", "id": "urg-03", "crew_name": "PendingCrew Mac 开发", "waiting": [{ "question": "公证用的 Apple 账号专用密码过期了，需要你重新生成一个", "waited_minutes": 1560 }], "human_mentions": 3, "issues": ["发版卡住"], "expect_level": [3, 4] },
    { "decision": "urgency", "id": "urg-04", "crew_name": "大绿豆后端", "waiting": [{ "question": "要不要回滚昨天的迁移", "waited_minutes": 10 }], "human_mentions": 2, "issues": ["线上支付回调全部返回服务器错误"], "expect_level": [4, 4] },
    { "decision": "urgency", "id": "urg-05", "crew_name": "大绿豆后端", "recent": ["看板：告警面板调好了", "群语音：断续修复已经合并"], "expect_level": [0, 1] },
    { "decision": "urgency", "id": "urg-06", "crew_name": "PendingCrew Mac 开发", "waiting": [{ "question": "配对码放在 Mac 上还是 iPhone 上", "waited_minutes": 3000 }, { "question": "crewd 用不用 gRPC", "waited_minutes": 1800 }], "human_mentions": 4, "expect_level": [3, 3] },
    { "decision": "urgency", "id": "urg-07", "crew_name": "个人博客", "waiting": [{ "question": "文章标题用哪一个", "waited_minutes": 180 }], "expect_level": [2, 2] },
    { "decision": "urgency", "id": "urg-08", "crew_name": "大绿豆后端", "human_mentions": 1, "issues": ["数据库磁盘快满了，可能很快写不进去"], "expect_level": [3, 4] }
  ]
}
```

- [ ] **Step 2: 写失败的测试**

`lib.rs`：

```rust
pub mod client;
pub mod decisions;
pub mod diag;
pub mod error;
pub mod eval;
pub mod jev;
pub mod key;
pub mod model;
pub mod prompts;
pub mod scrub;
pub mod shuffle;
```

`crates/jev/src/eval.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn j(id: &str, kind: DecisionKind, conf: f64, correct: bool) -> Judged {
        Judged {
            id: id.into(),
            kind,
            seed: 0,
            answer: if correct { "a".into() } else { "b".into() },
            confidence: Some(conf),
            correct,
            picked_first: Some(false),
            options: 4,
            nouls: vec![],
            error: None,
        }
    }

    #[test]
    fn the_dataset_is_well_formed_synthetic_and_balanced() {
        let text = include_str!("../eval/cases.zh.json");
        let ds = Dataset::parse(text).unwrap();
        assert_eq!(ds.cases.len(), 40);
        for kind in DecisionKind::ALL {
            let n = ds.cases.iter().filter(|c| c.kind() == kind).count();
            assert!(n >= 8, "{} 只有 {n} 条", kind.as_str());
        }
        // 仓库公开：只许合成数据（spec §12）。
        for needle in ["/Users/", "@gmail", "@qq.com", "sk-", "ghp_", "BEGIN"] {
            assert!(!text.contains(needle), "数据集里出现了 {needle}");
        }
    }

    #[test]
    fn validation_catches_bad_expectations() {
        let text = r#"{"crews":{"c":{"name":"x","members":[{"key":"captain","name":"机长","role":"机长","recency_rank":1},{"key":"a","name":"甲","role":"做甲","recency_rank":2}]}},
            "cases":[{"decision":"route_todo","id":"r","crew":"c","todo":"做甲","expect":["zzz"]}]}"#;
        let err = Dataset::parse(text).unwrap_err();
        assert!(err.contains("zzz"), "{err}");
        let dup = r#"{"crews":{"c":{"name":"x","members":[{"key":"a","name":"甲","role":"做甲","recency_rank":1}]}},
            "cases":[{"decision":"route_todo","id":"r","crew":"c","todo":"t","expect":["a"]},
                     {"decision":"route_todo","id":"r","crew":"c","todo":"t","expect":["a"]}]}"#;
        assert!(Dataset::parse(dup).unwrap_err().contains("重复"));
    }

    #[test]
    fn calibration_buckets_include_one_in_the_top_bucket() {
        let a = j("a", DecisionKind::RouteTodo, 1.0, true);
        let b = j("b", DecisionKind::RouteTodo, 0.25, false);
        let c = j("c", DecisionKind::RouteTodo, 0.1, true);
        let buckets = calibration(&[&a, &b, &c]);
        assert_eq!((buckets[4].n, buckets[4].correct), (1, 1));
        assert_eq!((buckets[1].n, buckets[1].correct), (1, 0));
        assert_eq!((buckets[0].n, buckets[0].correct), (1, 1));
    }

    #[test]
    fn threshold_is_the_lowest_that_meets_the_target() {
        let runs = [
            j("1", DecisionKind::RouteTodo, 0.95, true),
            j("2", DecisionKind::RouteTodo, 0.9, true),
            j("3", DecisionKind::RouteTodo, 0.8, true),
            j("4", DecisionKind::RouteTodo, 0.6, true),
            j("5", DecisionKind::RouteTodo, 0.4, false),
            j("6", DecisionKind::RouteTodo, 0.35, false),
        ];
        let refs: Vec<&Judged> = runs.iter().collect();
        let pick = recommend_threshold(&refs, 0.95).unwrap();
        assert_eq!(pick.threshold, 0.45);
        assert_eq!(pick.acted, 4);
        assert!((pick.coverage - 4.0 / 6.0).abs() < 1e-9);
        let bad = [
            j("1", DecisionKind::RouteTodo, 0.95, false),
            j("2", DecisionKind::RouteTodo, 0.95, true),
            j("3", DecisionKind::RouteTodo, 0.95, true),
        ];
        let refs: Vec<&Judged> = bad.iter().collect();
        assert_eq!(recommend_threshold(&refs, 0.95), None);
    }

    #[test]
    fn wake_threshold_maximises_member_accuracy_ties_toward_half() {
        let mut a = j("a", DecisionKind::Wake, 0.5, true);
        a.nouls = vec![(0.9, true), (0.62, true), (0.3, false), (0.05, false)];
        let pick = recommend_wake(&[&a]).unwrap();
        assert_eq!(pick.member_accuracy, 1.0);
        assert_eq!(pick.threshold, 0.5, "0.35–0.6 都全对，取离 0.5 最近的");
    }

    #[test]
    fn summary_counts_consistency_and_first_pick_bias() {
        let mut runs = vec![
            j("x", DecisionKind::RouteTodo, 0.9, true),
            j("x", DecisionKind::RouteTodo, 0.9, true),
            j("y", DecisionKind::RouteTodo, 0.9, true),
            j("y", DecisionKind::RouteTodo, 0.9, false),
        ];
        runs[0].picked_first = Some(true);
        let mut failed = j("z", DecisionKind::RouteTodo, 0.0, false);
        failed.error = Some("timeout".into());
        failed.confidence = None;
        runs.push(failed);
        let s = summarize(
            &runs,
            &[Duration::from_millis(900), Duration::from_millis(1200)],
            0.95,
        );
        let k = &s.kinds[0];
        assert_eq!(k.runs, 5);
        assert_eq!(k.errors, 1);
        assert_eq!(k.correct, 3);
        assert_eq!(k.consistent, (1, 2));
        assert_eq!(k.picked_first.map(|(h, n, _)| (h, n)), Some((1, 4)));
        assert!(s.markdown().contains("| route_todo | 5 | 1 | 75% | 1/2 |"));
        assert_eq!(s.latency_p50_ms, 1200);
    }
}
```

- [ ] **Step 3: 跑一遍确认失败**

Run: `cargo test -p crewd-jev --lib eval`
Expected: 编译失败（`Dataset` 等未定义）。

- [ ] **Step 4: 写实现**

`eval.rs`（测试模块上面）：

```rust
//! 中文实测集（spec §7.3「M5 先做实测」、§12「中文实测脚本单独跑」）：
//! 数据集格式、用例 → 决策输入、判对错、汇总（准确率、按置信度分桶、阈值推荐、选项顺序敏感度）。
//! 这里不碰网络；跑真 API 的是 `src/bin/jev-eval.rs`。
use crate::decisions::attribute::{self, AttributeInput, Candidate};
use crate::decisions::route::{self, RouteTodoInput};
use crate::decisions::urgency::{self, HumanRequest, UrgencyInput};
use crate::decisions::wake::{self, WakeInput};
use crate::decisions::{Built, DecisionKind, Member, Thresholds};
use crate::model::{Response, noul_confidence};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub crews: BTreeMap<String, CrewFixture>,
    pub cases: Vec<Case>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrewFixture {
    pub name: String,
    /// 含机长（key 固定为 "captain"）。派活时机长是固定选项，不当成员选项发。
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitingFixture {
    pub question: String,
    pub waited_minutes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum Case {
    RouteTodo {
        id: String,
        crew: String,
        todo: String,
        /// 可接受的选项名（任一即对）。
        expect: Vec<String>,
    },
    Wake {
        id: String,
        crew: String,
        /// 这个群的成员 key。
        channel: Vec<String>,
        #[serde(default)]
        recent: Vec<String>,
        message: String,
        /// 必须叫醒的成员。
        expect: Vec<String>,
        /// 叫不叫都算对的成员。
        #[serde(default)]
        optional: Vec<String>,
    },
    Attribute {
        id: String,
        message: String,
        candidates: Vec<Candidate>,
        expect: Vec<String>,
    },
    Urgency {
        id: String,
        crew_name: String,
        #[serde(default)]
        waiting: Vec<WaitingFixture>,
        #[serde(default)]
        human_mentions: usize,
        #[serde(default)]
        issues: Vec<String>,
        #[serde(default)]
        recent: Vec<String>,
        /// 可接受的最可能级别，闭区间 [低, 高]。
        expect_level: [u8; 2],
    },
}

impl Case {
    pub fn id(&self) -> &str {
        match self {
            Case::RouteTodo { id, .. }
            | Case::Wake { id, .. }
            | Case::Attribute { id, .. }
            | Case::Urgency { id, .. } => id,
        }
    }

    pub fn kind(&self) -> DecisionKind {
        match self {
            Case::RouteTodo { .. } => DecisionKind::RouteTodo,
            Case::Wake { .. } => DecisionKind::Wake,
            Case::Attribute { .. } => DecisionKind::Attribute,
            Case::Urgency { .. } => DecisionKind::Urgency,
        }
    }
}

impl Dataset {
    pub fn parse(text: &str) -> Result<Dataset, String> {
        let ds: Dataset = serde_json::from_str(text).map_err(|e| format!("数据集解析失败：{e}"))?;
        ds.validate()?;
        Ok(ds)
    }

    fn crew(&self, key: &str) -> Result<&CrewFixture, String> {
        self.crews
            .get(key)
            .ok_or_else(|| format!("没有机组 {key:?}"))
    }

    /// 构造一次请求。种子只影响 choice 的选项顺序。
    pub fn build_case(&self, case: &Case, seed: u64) -> Result<Built, String> {
        match case {
            Case::RouteTodo { crew, todo, .. } => {
                let crew = self.crew(crew)?;
                let input = RouteTodoInput {
                    todo: todo.clone(),
                    members: crew
                        .members
                        .iter()
                        .filter(|m| m.key != route::CAPTAIN)
                        .cloned()
                        .collect(),
                };
                route::build(&input, seed)
            }
            Case::Wake {
                crew,
                channel,
                recent,
                message,
                ..
            } => {
                let crew = self.crew(crew)?;
                let members = channel
                    .iter()
                    .map(|k| {
                        crew.members
                            .iter()
                            .find(|m| &m.key == k)
                            .cloned()
                            .ok_or_else(|| format!("群成员 {k:?} 不在机组里"))
                    })
                    .collect::<Result<Vec<Member>, String>>()?;
                wake::build(&WakeInput {
                    message: message.clone(),
                    recent: recent.clone(),
                    members,
                })
            }
            Case::Attribute {
                message,
                candidates,
                ..
            } => attribute::build(
                &AttributeInput {
                    message: message.clone(),
                    candidates: candidates.clone(),
                },
                seed,
            ),
            Case::Urgency {
                crew_name,
                waiting,
                human_mentions,
                issues,
                recent,
                ..
            } => urgency::build(&UrgencyInput {
                crew_name: crew_name.clone(),
                waiting_on_human: waiting
                    .iter()
                    .map(|w| HumanRequest {
                        text: w.question.clone(),
                        waited: Duration::from_secs(w.waited_minutes * 60),
                    })
                    .collect(),
                human_mentions: *human_mentions,
                issues: issues.clone(),
                recent: recent.clone(),
            }),
        }
    }

    /// 用例自洽：id 唯一、引用的机组 / 成员存在、期望答案都在发出的选项里、每条都能构造。
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        for case in &self.cases {
            let id = case.id();
            if !ids.insert(id.to_string()) {
                return Err(format!("用例 id 重复：{id}"));
            }
            let built = self
                .build_case(case, 0)
                .map_err(|e| format!("{id} 构造失败：{e}"))?;
            let allowed: BTreeSet<&str> = built.options.iter().map(String::as_str).collect();
            let expected: Vec<&String> = match case {
                Case::RouteTodo { expect, .. } | Case::Attribute { expect, .. } => {
                    if expect.is_empty() {
                        return Err(format!("{id} 没有期望答案"));
                    }
                    expect.iter().collect()
                }
                Case::Wake {
                    expect, optional, ..
                } => expect.iter().chain(optional.iter()).collect(),
                Case::Urgency { expect_level, .. } => {
                    let [lo, hi] = *expect_level;
                    if lo > hi || usize::from(hi) >= urgency::LEVELS {
                        return Err(format!("{id} 的 expect_level 不合法"));
                    }
                    vec![]
                }
            };
            for e in expected {
                if !allowed.contains(e.as_str()) {
                    return Err(format!("{id} 期望的 {e:?} 不在选项里"));
                }
            }
        }
        Ok(())
    }

    pub fn case(&self, id: &str) -> Option<&Case> {
        self.cases.iter().find(|c| c.id() == id)
    }
}

/// 一次运行（一条用例 × 一个种子）的判定。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Judged {
    pub id: String,
    pub kind: DecisionKind,
    pub seed: u64,
    /// 模型的答案（选项名 / 叫醒集合 / 级别），出错时为空。
    pub answer: String,
    pub confidence: Option<f64>,
    pub correct: bool,
    /// choice：选中的是不是排第一的选项。
    pub picked_first: Option<bool>,
    pub options: usize,
    /// wake：每个非 optional 成员的 (p, 应不应该叫醒)。
    pub nouls: Vec<(f64, bool)>,
    pub error: Option<String>,
}

impl Judged {
    pub fn failed(case: &Case, seed: u64, error: String) -> Judged {
        Judged {
            id: case.id().to_string(),
            kind: case.kind(),
            seed,
            answer: String::new(),
            confidence: None,
            correct: false,
            picked_first: None,
            options: 0,
            nouls: vec![],
            error: Some(error),
        }
    }
}

/// 判对错。看的是模型的答案（`Verdict::best`），不看阈值——阈值正是要从这里推出来的。
pub fn judge(case: &Case, seed: u64, built: &Built, response: &Response) -> Judged {
    let th = Thresholds::default();
    let mut j = Judged::failed(case, seed, String::new());
    j.error = None;
    j.options = built.options.len();
    match case {
        Case::RouteTodo { expect, .. } => {
            let v = route::interpret(built, response, &th);
            match v.best {
                Some(t) => {
                    let key = route::target_key(&t).to_string();
                    j.correct = expect.contains(&key);
                    j.picked_first = Some(built.options.first() == Some(&key));
                    j.answer = key;
                    j.confidence = v.confidence;
                }
                None => j.error = Some(format!("{:?}", v.outcome)),
            }
        }
        Case::Attribute { expect, .. } => {
            let v = attribute::interpret(built, response, &th);
            match v.best {
                Some(a) => {
                    let key = attribute::attribution_key(&a).to_string();
                    j.correct = expect.contains(&key);
                    j.picked_first = Some(built.options.first() == Some(&key));
                    j.answer = key;
                    j.confidence = v.confidence;
                }
                None => j.error = Some(format!("{:?}", v.outcome)),
            }
        }
        Case::Wake {
            expect, optional, ..
        } => match wake::probabilities(built, response) {
            Ok(ps) => {
                let woken: BTreeSet<&str> = ps
                    .iter()
                    .filter(|(_, p)| *p >= th.wake_yes)
                    .map(|(k, _)| k.as_str())
                    .collect();
                j.correct = wake_set_ok(&woken, expect, optional);
                j.answer = woken.into_iter().collect::<Vec<_>>().join(",");
                j.nouls = ps
                    .iter()
                    .filter(|(k, _)| !optional.contains(k))
                    .map(|(k, p)| (*p, expect.contains(k)))
                    .collect();
                j.confidence = ps.iter().map(|(_, p)| noul_confidence(*p)).reduce(f64::min);
            }
            Err(reason) => j.error = Some(reason.label()),
        },
        Case::Urgency { expect_level, .. } => {
            let v = urgency::interpret(built, response, &th);
            match v.best {
                Some(u) => {
                    j.correct = (expect_level[0]..=expect_level[1]).contains(&u.level);
                    j.answer = format!("{} ({:.2})", u.level, u.score);
                    j.confidence = v.confidence;
                }
                None => j.error = Some(format!("{:?}", v.outcome)),
            }
        }
    }
    j
}

fn wake_set_ok(woken: &BTreeSet<&str>, expect: &[String], optional: &[String]) -> bool {
    expect.iter().all(|e| woken.contains(e.as_str()))
        && woken
            .iter()
            .all(|w| expect.iter().any(|e| e == w) || optional.iter().any(|o| o == w))
}

/// 置信度分桶（左闭右开，最后一桶含 1.0）。
pub const BUCKETS: [(f64, f64); 5] = [
    (0.0, 0.25),
    (0.25, 0.5),
    (0.5, 0.75),
    (0.75, 0.9),
    (0.9, 1.0),
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BucketStat {
    pub lo: f64,
    pub hi: f64,
    pub n: usize,
    pub correct: usize,
}

pub fn calibration(judged: &[&Judged]) -> Vec<BucketStat> {
    BUCKETS
        .iter()
        .enumerate()
        .map(|(i, &(lo, hi))| {
            let last = i == BUCKETS.len() - 1;
            let inside: Vec<&&Judged> = judged
                .iter()
                .filter(|j| {
                    j.confidence
                        .is_some_and(|c| c >= lo && (c < hi || (last && c <= hi)))
                })
                .collect();
            BucketStat {
                lo,
                hi,
                n: inside.len(),
                correct: inside.iter().filter(|j| j.correct).count(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pick {
    pub threshold: f64,
    /// 置信度 ≥ 阈值的那部分里答对的比例。
    pub precision: f64,
    /// 置信度 ≥ 阈值的比例（= Jev 自己执行、不打扰机长的比例）。
    pub coverage: f64,
    pub acted: usize,
}

fn grid(lo: f64, hi: f64) -> Vec<f64> {
    let steps = ((hi - lo) / 0.05).round() as usize;
    (0..=steps)
        .map(|i| ((lo + 0.05 * i as f64) * 100.0).round() / 100.0)
        .collect()
}

/// choice / score：在 0.30–0.90（步长 0.05）里取最小的阈值，使「置信度 ≥ 阈值」的那部分准确率 ≥ target，
/// 且至少有 3 条。没有满足的 → None（这个决策在中文上不该自动执行）。
pub fn recommend_threshold(judged: &[&Judged], target: f64) -> Option<Pick> {
    let scored: Vec<&&Judged> = judged.iter().filter(|j| j.confidence.is_some()).collect();
    if scored.is_empty() {
        return None;
    }
    for t in grid(0.30, 0.90) {
        let acted: Vec<&&&Judged> = scored
            .iter()
            .filter(|j| j.confidence.unwrap_or(0.0) >= t)
            .collect();
        if acted.len() < 3 {
            continue;
        }
        let precision = acted.iter().filter(|j| j.correct).count() as f64 / acted.len() as f64;
        if precision >= target {
            return Some(Pick {
                threshold: t,
                precision,
                coverage: acted.len() as f64 / scored.len() as f64,
                acted: acted.len(),
            });
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WakePick {
    pub threshold: f64,
    /// 每个成员单独判对（p ≥ 阈值 ⇔ 应该叫醒）的比例。
    pub member_accuracy: f64,
    pub judgements: usize,
}

/// wake：在 0.20–0.80 里取让「逐成员判对率」最高的阈值；并列取离 0.5 最近的。
pub fn recommend_wake(judged: &[&Judged]) -> Option<WakePick> {
    let all: Vec<(f64, bool)> = judged
        .iter()
        .flat_map(|j| j.nouls.iter().copied())
        .collect();
    if all.is_empty() {
        return None;
    }
    let mut best: Option<WakePick> = None;
    for t in grid(0.20, 0.80) {
        let right = all.iter().filter(|(p, e)| (*p >= t) == *e).count();
        let acc = right as f64 / all.len() as f64;
        let better = match &best {
            None => true,
            Some(b) => {
                acc > b.member_accuracy + 1e-12
                    || ((acc - b.member_accuracy).abs() <= 1e-12
                        && (t - 0.5).abs() < (b.threshold - 0.5).abs())
            }
        };
        if better {
            best = Some(WakePick {
                threshold: t,
                member_accuracy: acc,
                judgements: all.len(),
            });
        }
    }
    best
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KindSummary {
    pub kind: DecisionKind,
    pub runs: usize,
    pub errors: usize,
    pub correct: usize,
    pub accuracy: f64,
    pub buckets: Vec<BucketStat>,
    pub pick: Option<Pick>,
    pub wake_pick: Option<WakePick>,
    /// 多个种子的用例里，各种子答案完全一致的条数 / 总条数。
    pub consistent: (usize, usize),
    /// choice：选中排第一的次数 / 次数，以及随机情况下的期望比例（平均 1/选项数）。
    pub picked_first: Option<(usize, usize, f64)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub target_precision: f64,
    pub kinds: Vec<KindSummary>,
    pub latency_p50_ms: u64,
    pub latency_p95_ms: u64,
}

pub fn percentile_ms(latencies: &[Duration], pct: f64) -> u64 {
    if latencies.is_empty() {
        return 0;
    }
    let mut ms: Vec<u64> = latencies.iter().map(|d| d.as_millis() as u64).collect();
    ms.sort_unstable();
    let rank = ((pct / 100.0) * (ms.len() - 1) as f64).round() as usize;
    ms[rank.min(ms.len() - 1)]
}

pub fn summarize(judged: &[Judged], latencies: &[Duration], target_precision: f64) -> Summary {
    let kinds = DecisionKind::ALL
        .iter()
        .filter_map(|&kind| {
            let runs: Vec<&Judged> = judged.iter().filter(|j| j.kind == kind).collect();
            if runs.is_empty() {
                return None;
            }
            let ok: Vec<&Judged> = runs.iter().copied().filter(|j| j.error.is_none()).collect();
            let correct = ok.iter().filter(|j| j.correct).count();
            let mut by_id: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            let mut seeds: BTreeMap<&str, usize> = BTreeMap::new();
            for j in &ok {
                by_id.entry(&j.id).or_default().insert(&j.answer);
                *seeds.entry(&j.id).or_default() += 1;
            }
            let multi: Vec<&str> = seeds
                .iter()
                .filter(|(_, n)| **n >= 2)
                .map(|(id, _)| *id)
                .collect();
            let consistent = multi.iter().filter(|id| by_id[*id].len() == 1).count();
            let firsts: Vec<&Judged> = ok
                .iter()
                .copied()
                .filter(|j| j.picked_first.is_some())
                .collect();
            let picked_first = (!firsts.is_empty()).then(|| {
                let hits = firsts
                    .iter()
                    .filter(|j| j.picked_first == Some(true))
                    .count();
                let expected = firsts
                    .iter()
                    .map(|j| 1.0 / j.options.max(1) as f64)
                    .sum::<f64>()
                    / firsts.len() as f64;
                (hits, firsts.len(), expected)
            });
            Some(KindSummary {
                kind,
                runs: runs.len(),
                errors: runs.len() - ok.len(),
                correct,
                accuracy: if ok.is_empty() {
                    0.0
                } else {
                    correct as f64 / ok.len() as f64
                },
                buckets: calibration(&ok),
                pick: if kind == DecisionKind::Wake {
                    None
                } else {
                    recommend_threshold(&ok, target_precision)
                },
                wake_pick: if kind == DecisionKind::Wake {
                    recommend_wake(&ok)
                } else {
                    None
                },
                consistent: (consistent, multi.len()),
                picked_first,
            })
        })
        .collect();
    Summary {
        target_precision,
        kinds,
        latency_p50_ms: percentile_ms(latencies, 50.0),
        latency_p95_ms: percentile_ms(latencies, 95.0),
    }
}

impl Summary {
    /// 贴进 spec 的 Markdown。
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "目标准确率 {:.0}%；延迟 p50 {} ms / p95 {} ms（含网络与重试）。\n",
            self.target_precision * 100.0,
            self.latency_p50_ms,
            self.latency_p95_ms
        );
        let _ = writeln!(
            out,
            "| 决策 | 次数 | 出错 | 准确率 | 种子间一致 | 选中排第一（随机期望） | 推荐阈值 |"
        );
        let _ = writeln!(out, "|---|---|---|---|---|---|---|");
        for k in &self.kinds {
            let first = match k.picked_first {
                Some((hits, n, expected)) => format!("{hits}/{n}（{:.0}%）", expected * 100.0),
                None => "—".into(),
            };
            let pick = match (&k.pick, &k.wake_pick) {
                (Some(p), _) => format!(
                    "{:.2}（准确 {:.0}%，自动执行 {:.0}%）",
                    p.threshold,
                    p.precision * 100.0,
                    p.coverage * 100.0
                ),
                (None, Some(w)) => format!(
                    "noul {:.2}（逐成员判对 {:.0}%，{} 次判断）",
                    w.threshold,
                    w.member_accuracy * 100.0,
                    w.judgements
                ),
                (None, None) => "无（达不到目标，全交机长）".into(),
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} | {:.0}% | {}/{} | {} | {} |",
                k.kind.as_str(),
                k.runs,
                k.errors,
                k.accuracy * 100.0,
                k.consistent.0,
                k.consistent.1,
                first,
                pick
            );
        }
        let _ = writeln!(out, "\n按置信度分桶（答对 / 次数）：\n");
        let _ = writeln!(
            out,
            "| 决策 | [0,.25) | [.25,.5) | [.5,.75) | [.75,.9) | [.9,1] |"
        );
        let _ = writeln!(out, "|---|---|---|---|---|---|");
        for k in &self.kinds {
            let cells: Vec<String> = k
                .buckets
                .iter()
                .map(|b| format!("{}/{}", b.correct, b.n))
                .collect();
            let _ = writeln!(out, "| {} | {} |", k.kind.as_str(), cells.join(" | "));
        }
        out
    }
}
```

`crates/jev/src/bin/jev-eval.rs`：

```rust
//! jev-eval：用真 API 跑中文实测集，报准确率、按置信度分桶、推荐阈值。
//! opt-in：check.sh 不跑它，CI 也不跑。会花钱（每条用例 × 每个种子一次请求）。
//!
//! key：环境变量 JEV_API_KEY；没有就在 macOS 上用 `security find-generic-password` 读钥匙串
//! （service=pendingcrew-jev，account=default）。key 不打印、不落盘。
//!
//! 用法：cargo run -p crewd-jev --bin jev-eval -- [--cases PATH] [--seeds N] [--only route_todo|wake|attribute|urgency]
//!       [--out REPORT.json] [--record DIR] [--base-url URL] [--model NAME] [--target 0.95]
use crewd_jev::client::{ApiKey, Client, ClientConfig};
use crewd_jev::eval::{Dataset, Judged, judge, summarize};
use crewd_jev::key::{KEYCHAIN_ACCOUNT, KEYCHAIN_SERVICE};
use crewd_jev::model::{DEFAULT_BASE_URL, DEFAULT_MODEL};
use crewd_jev::shuffle::seed_from;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const DEFAULT_CASES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/eval/cases.zh.json");
/// 两次请求之间歇一下，别自己把自己限流。
const PAUSE: Duration = Duration::from_millis(200);

struct Args {
    cases: PathBuf,
    seeds: u64,
    only: Option<String>,
    out: Option<PathBuf>,
    record: Option<PathBuf>,
    base_url: String,
    model: String,
    target: f64,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        cases: PathBuf::from(DEFAULT_CASES),
        seeds: 2,
        only: None,
        out: None,
        record: None,
        base_url: DEFAULT_BASE_URL.into(),
        model: DEFAULT_MODEL.into(),
        target: 0.95,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} 缺参数"));
        match flag.as_str() {
            "--cases" => args.cases = PathBuf::from(value()?),
            "--seeds" => args.seeds = value()?.parse().map_err(|e| format!("--seeds：{e}"))?,
            "--only" => args.only = Some(value()?),
            "--out" => args.out = Some(PathBuf::from(value()?)),
            "--record" => args.record = Some(PathBuf::from(value()?)),
            "--base-url" => args.base_url = value()?,
            "--model" => args.model = value()?,
            "--target" => args.target = value()?.parse().map_err(|e| format!("--target：{e}"))?,
            other => return Err(format!("不认识的参数 {other}")),
        }
    }
    if args.seeds == 0 {
        return Err("--seeds 至少 1".into());
    }
    Ok(args)
}

fn read_key() -> Result<ApiKey, String> {
    if let Ok(v) = std::env::var("JEV_API_KEY") {
        return ApiKey::new(&v).ok_or_else(|| "JEV_API_KEY 是空的或含空白".to_string());
    }
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                KEYCHAIN_ACCOUNT,
                "-w",
            ])
            .output()
            .map_err(|e| format!("跑 security 失败：{e}"))?;
        if out.status.success() {
            return ApiKey::new(&String::from_utf8_lossy(&out.stdout))
                .ok_or_else(|| "钥匙串里的 key 是空的".to_string());
        }
    }
    Err(format!(
        "没有 key：设 JEV_API_KEY，或在钥匙串里放 service={KEYCHAIN_SERVICE} account={KEYCHAIN_ACCOUNT}"
    ))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jev-eval: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    let text = std::fs::read_to_string(&args.cases)
        .map_err(|e| format!("读 {} 失败：{e}", args.cases.display()))?;
    let dataset = Dataset::parse(&text)?;
    let client = Client::new(ClientConfig::new(args.base_url.clone(), read_key()?))?;
    if let Some(dir) = &args.record {
        std::fs::create_dir_all(dir).map_err(|e| format!("建 {} 失败：{e}", dir.display()))?;
    }
    let cases: Vec<_> = dataset
        .cases
        .iter()
        .filter(|c| args.only.as_deref().is_none_or(|k| c.kind().as_str() == k))
        .collect();
    eprintln!(
        "jev-eval: {} 条用例 × {} 个种子 → {}",
        cases.len(),
        args.seeds,
        client.endpoint()
    );
    let mut judged: Vec<Judged> = Vec::new();
    let mut latencies = Vec::new();
    // jev-latest 是别名；记下实际回答的模型版本（spec 里要写的是它）。
    let mut models = std::collections::BTreeSet::new();
    for case in cases {
        for s in 0..args.seeds {
            let seed = seed_from(&[case.id(), &s.to_string()]);
            let built = dataset.build_case(case, seed)?;
            let (result, report) = client.evaluate_raw(&built.request(&args.model)).await;
            latencies.push(report.latency);
            let j = match result {
                Ok((response, raw)) => {
                    models.insert(response.model.clone());
                    if let Some(dir) = &args.record {
                        // 原文照存：夹具要的是真实应答的形状，不是我们类型再序列化一遍。
                        let path = dir.join(format!("{}-s{s}.json", case.id()));
                        std::fs::write(&path, raw)
                            .map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
                    }
                    judge(case, seed, &built, &response)
                }
                Err(e) => Judged::failed(case, seed, e.to_string()),
            };
            eprintln!(
                "{:<10} s{s} {} {:<24} conf={} {}ms{}",
                j.id,
                if j.correct { "✓" } else { "✗" },
                j.answer,
                j.confidence.map_or("-".into(), |c| format!("{c:.2}")),
                report.latency.as_millis(),
                j.error
                    .as_deref()
                    .map_or(String::new(), |e| format!(" error={e}")),
            );
            judged.push(j);
            tokio::time::sleep(PAUSE).await;
        }
    }
    let summary = summarize(&judged, &latencies, args.target);
    let models: Vec<String> = models.into_iter().collect();
    println!("实际模型：{}\n", models.join(", "));
    println!("{}", summary.markdown());
    if let Some(out) = &args.out {
        let report = serde_json::json!({
            "model": args.model,
            "resolved_models": models,
            "cases": args.cases.file_name().map(|n| n.to_string_lossy().into_owned()),
            "seeds": args.seeds,
            "summary": summary,
            "runs": judged,
        });
        let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        std::fs::write(out, text).map_err(|e| format!("写 {} 失败：{e}", out.display()))?;
    }
    Ok(())
}
```

`crewd/README.md`：在「终端核心」那一行后面加两行：

```markdown
- Jev 客户端：`crates/jev`（派活 / 叫醒 / 归属 / 紧急度四个判断；发前脱敏；本机诊断日志不记 key）。给 Jev 看的文字全在 `crates/jev/prompts/*.md`
- Jev 评测（真 API、花钱、不进 check.sh）：`cargo run -p crewd-jev --bin jev-eval -- --seeds 2 --out crates/jev/eval/results/<日期>.json`；key 取 `JEV_API_KEY`，否则钥匙串 service `pendingcrew-jev` account `default`。改了 `prompts/` 的措辞或 `eval/cases.zh.json` 后重跑，读数记进 spec 的「M5a 交付记录与中文实测」一节
```

- [ ] **Step 5: 跑通**

Run: `cargo test -p crewd-jev --lib`
Expected: `61 passed; 1 ignored`（+ eval 6）。`the_dataset_is_well_formed_synthetic_and_balanced` 会把 40 条用例每条都构造一遍并核对期望答案都在发出的选项里（写计划时核过全绿）。

- [ ] **Step 6: 干跑 `jev-eval`（不花钱：假 key、打到没人监听的端口）**

```bash
JEV_API_KEY=dummy-not-a-real-key cargo run -p crewd-jev --bin jev-eval -- --base-url http://127.0.0.1:9 --only urgency --seeds 1
echo "exit=$?"
```

Expected: stderr 每条一行 `urg-0x s0 ✗ ... error=transport: ...`（共 8 行）；stdout 的表里有一行 `| urgency | 8 | 8 | 0% | 0/0 | — | 无（达不到目标，全交机长） |`，分桶行全是 `0/0`；`exit=0`。用假 key 是为了这一步绝不读钥匙串、绝不外发真 key。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/jev/eval/cases.zh.json crates/jev/src/eval.rs crates/jev/src/bin/jev-eval.rs crates/jev/src/lib.rs README.md
git diff --cached --stat
git commit -m "crewd-jev: 40-case synthetic Chinese evaluation set and opt-in jev-eval (accuracy, calibration, threshold pick)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 9: 真跑一次实测，录夹具，读数写进 spec

人已同意用钥匙串里的 key 跑这一次（约 80 次请求：40 条 × 2 个种子）。这是本计划里唯一一次外发。

**Files:**
- Create: `crewd/crates/jev/eval/results/<YYYY-MM-DD>.json`（跑的那天的日期）、`crewd/crates/jev/tests/fixtures/recorded/{route-01-s0,wake-02-s0,attr-02-s0,urg-04-s0}.json`、`crewd/crates/jev/tests/recorded.rs`
- Modify: `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`；视读数而定 `crewd/crates/jev/src/decisions/mod.rs`（`Thresholds::default` 与 `defaults_are_pinned`）

**Interfaces:**
- Consumes: Task 8 的 `jev-eval`、`model::{Response, Answer}`。
- Produces: 4 份真实应答原文夹具；spec 新增一节；（可能）新的 `Thresholds::default()`。

- [ ] **Step 1: 确认 key 在、不打印它**

```bash
security find-generic-password -s pendingcrew-jev -a default >/dev/null 2>&1 && echo present || echo missing
env | grep -c '^JEV_API_KEY=' ; true
```

Expected: `present`；第二行 `0`（确保用的是钥匙串，不是哪个残留的环境变量）。`missing` → 停下，在群里告诉人「钥匙串里没有 pendingcrew-jev/default」，不要自己找别的 key。

- [ ] **Step 2: 跑**

```bash
mkdir -p crates/jev/eval/results target/jev-recorded
DAY=$(date +%F)
env -u JEV_API_KEY cargo run --release -p crewd-jev --bin jev-eval -- --seeds 2 --record target/jev-recorded --out crates/jev/eval/results/$DAY.json > target/jev-eval.md 2> target/jev-eval.log
echo "exit=$?"
cat target/jev-eval.md
tail -5 target/jev-eval.log
```

Expected: `exit=0`；`target/jev-eval.md` 第一行 `实际模型：jev-1.13.x`（或更新的版本），下面两张表（总表 4 行、分桶表 4 行）；`target/jev-eval.log` 80 行逐条结果。

- 出错列不是 0 时看 `target/jev-eval.log` 的 `error=`：
  - `unauthorized (401)` → key 失效。停下，在群里告诉人，不要重试。
  - `rejected (422)` → 我们的请求形状不合官方 schema。读 log 里的 body，回到 Task 1 / 5 / 6 对应位置改，**先加一条能复现的单测**，再重跑本步。
  - `rate limited` / `overloaded` / `timeout` 零星几条 → 照常往下；超过 8 条就隔几分钟重跑一次整轮，两次都记进新增那一节。

- [ ] **Step 3: 核对报告里没有 key**

```bash
security find-generic-password -s pendingcrew-jev -a default -w | grep -F -c -f /dev/stdin target/jev-eval.log target/jev-eval.md crates/jev/eval/results/*.json target/jev-recorded/*.json ; true
```

Expected: 每个文件都是 `:0`。（key 经管道当成 grep 的模式文件，不进任何进程的 argv。）这把尺子只在 Step 1 是 `present` 时有效——`security` 读不出东西时模式为空，全是 0 也说明不了什么。有任何非 0 → 停下，删掉那个文件，回报。

- [ ] **Step 4: 录夹具 + 测试**

```bash
mkdir -p crates/jev/tests/fixtures/recorded
for f in route-01-s0 wake-02-s0 attr-02-s0 urg-04-s0; do cp target/jev-recorded/$f.json crates/jev/tests/fixtures/recorded/; done
cat crates/jev/tests/fixtures/recorded/*.json
```

Expected: 四个文件都只有 `model`、`answers`、`usage`（或官方新增的、不含我们输入的字段）。若应答里回显了我们的 state（官方文档没说会），停下回报，不提交。

`crates/jev/tests/recorded.rs`：

```rust
//! Task 9 用 `jev-eval --record` 录下的真实应答原文（每种决策一条）：我们的类型必须能解析，
//! 三种答案类型都要出现过（spec §12「录制真实响应做夹具」）。夹具缺了必须失败，不能静默跳过。
use crewd_jev::model::{Answer, Response};
use std::collections::BTreeSet;
use std::path::Path;

const RECORDED: [&str; 4] = [
    "route-01-s0.json",
    "wake-02-s0.json",
    "attr-02-s0.json",
    "urg-04-s0.json",
];

#[test]
fn recorded_real_responses_parse_with_our_types() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recorded");
    let mut kinds = BTreeSet::new();
    for name in RECORDED {
        let path = dir.join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("缺夹具 {}：{e}", path.display()));
        let response: Response =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} 解析失败：{e}"));
        assert!(
            response.model.starts_with("jev-"),
            "{name}: {}",
            response.model
        );
        for answer in response.answers.values() {
            kinds.insert(match answer {
                Answer::Choice { confidence, .. } => {
                    assert!((0.0..=1.0).contains(confidence));
                    "choice"
                }
                Answer::Score { confidence, .. } => {
                    assert!((0.0..=1.0).contains(confidence));
                    "score"
                }
                Answer::Noul { noul } => {
                    assert!((0.0..=1.0).contains(noul));
                    "noul"
                }
            });
        }
    }
    assert_eq!(kinds, BTreeSet::from(["choice", "noul", "score"]));
}
```

Run: `cargo test -p crewd-jev --test recorded`
Expected: `1 passed`。若解析失败：真实形状与文档不符——这正是夹具要抓的。按真实形状改 `model.rs`（加单测），在新增那一节记下「文档写 X，实际是 Y」。

- [ ] **Step 5: 按读数定阈值**

看 `target/jev-eval.md` 总表最后一列：
- route_todo、attribute、urgency 三行**都有**推荐值，wake 行有 noul 推荐值 → 把 `crates/jev/src/decisions/mod.rs` 里 `Thresholds::default()` 改成：`choice_min` = route_todo 与 attribute 两个推荐值中**较大**的那个（两者共用一条线），`score_min` = urgency 的推荐值，`wake_yes` = wake 的推荐值；同步改 `defaults_are_pinned` 的期望与 `Thresholds` 文档注释里的「默认值 = spec §7.3 的 0.5；M5a Task 9 实测后按读数改（改了要同步 `defaults_are_pinned`）。」为「默认值 = M5a 实测（spec「M5a 交付记录与中文实测」）。」
- 有任何一行是「无」→ **不改代码**，默认值留 0.5；在新增那一节写明哪个决策达不到 95%，并在 spec §13 加一条待人拍板的问题（见 Step 6 模板）。是否在中文上让这个决策自动执行，是产品决定，不是实现者能定的。

- [ ] **Step 6: 写 spec**

改 `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md` 三处。**节号**：新增那一节的编号 = 落笔时 spec 里最大的节号 + 1（写计划时最大是 §17「机组内多群」；M2b 的交付记录若先合进来，可能已经占了 §18），下面写作 `§N`，落笔时三处一起换成实际数字；§13 新问题的号同理写作 `Q<下一个号>`（写计划时最后一条是 Q12）。

(a) §3.3 里这一行：

```
- **M5 Jev 调度**（§7）：先跑中文实测集定阈值。
```

改成：

```
- **M5 Jev 调度**（§7）：**M5a** 客户端 + 中文实测（已交付，§N）→ **M5b** 接进调度（依赖 M3 的群与成员、M4 的工作项）。
```

(b) §7.3 最后一条里的「阈值（低置信度线，默认 0.5；noul 阈值）由实测定。」改成「阈值（低置信度线，默认 0.5；noul 阈值）由实测定——M5a 读数与取值见 §N。」

(c) 文末追加一节（`<…>` 处照填，数字与表格**原样**取自 `target/jev-eval.md` 与 `crates/jev/eval/results/<日期>.json`，不手抄、不四舍五入）：

```markdown
## N. M5a 交付记录与中文实测（分支 crewd-m5a，计划 `docs/superpowers/plans/2026-10-06-crewd-m5a-jev-client.md`）

已交付：`crewd/crates/jev`（crewd-jev）——System One 线上类型（choice 选项保序上线）、凭据脱敏（7 类形状，幂等，提交号 / UUID 不遮）、异步客户端（reqwest + rustls；Bearer；单次 5s / 总 12s；429 / 529 / 5xx / 连接错误退避重试，尊重 Retry-After；401 / 422 不重试；只收 https）、key 存放（macOS 钥匙串 `pendingcrew-jev`/`default`，其他平台 0600 文件，权限宽就拒读）、四个决策的构造与判定（种子打乱、名单截 15、量换词、选项校验）、门面 `Jev`（未配置不联网）、本机 JSONL 诊断日志（0600、5 MiB 轮转、不记 key）、40 条合成中文实测集与 `jev-eval`。给 Jev 看的文字全在 `crates/jev/prompts/*.md`。

实测（<日期>，`jev-latest` 实际解析为 <实际模型>，40 条 × 2 个种子，报告 `crewd/crates/jev/eval/results/<日期>.json`）：

<把 target/jev-eval.md 从「目标准确率」那一行起整段贴在这里>

取值：<二选一照实写>
- 已改默认阈值：choice_min = <值>（route_todo <值> / attribute <值> 取大），score_min = <值>，wake_yes = <值>。
- 未改默认阈值（仍 0.5）：<哪个决策> 在任何阈值下都达不到 95%（最高 <值>），见 §13 Q<号>。

读数的边界：
- 用例只有 40 条、每个决策 8–13 条，阈值是粗估；「种子间一致」只说明同一输入换选项顺序答案稳不稳，不说明对不对。
- 用例是写计划的人按 PendingCrew 用法编的，措辞比真实群消息规整；真实分布下的准确率只会更低。
- 「选中排第一」与随机期望比，只在两个种子、少量样本上看偏向，不足以证明没有偏向。
- 延迟含网络与重试，量的是本机到官方端点这一条路。
- <Step 2 里出现过的错误、重跑，照实写；没有就写「无出错」>
```

只有 Step 5 走了「有『无』」那一支时，才在 §13 末尾追加（`Q<号>` 换成实际号）：

```markdown
- Q<号> **[待人拍板]** Jev 在中文上 <哪个决策> 达不到 95% 准确（M5a 实测最高 <值>，§N）。选项：A 仍按 0.5 自动执行（错了可撤回）；B 这个决策全交机长、只记日志，等扩大用例集或新模型后重测；C 改提示词后重测。
```

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/jev/eval/results/ crates/jev/tests/fixtures/recorded/route-01-s0.json crates/jev/tests/fixtures/recorded/wake-02-s0.json crates/jev/tests/fixtures/recorded/attr-02-s0.json crates/jev/tests/fixtures/recorded/urg-04-s0.json crates/jev/tests/recorded.rs ../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md
# 只有 Step 5 改了阈值时再加：git add crates/jev/src/decisions/mod.rs
git diff --cached --stat
git commit -m "crewd-jev: first live Chinese evaluation against jev-latest; recorded real responses as fixtures; spec delivery record

Co-Authored-By: Claude <noreply@anthropic.com>"
```

Expected（`--stat`）：一个结果 JSON、四个夹具、一个测试文件、spec，（可能）`decisions/mod.rs`。`target/` 下的东西不提交（gitignored）。

## 待定（不阻塞本计划，M5b 前要定）

- **crewd 运行时读钥匙串的访问控制**：条目若由 `security` 或 Mac app 建，crewd（尤其是每次重编、未签名的开发版）读它会弹系统授权框；GUI 拉起时还算有人点，后台恢复时没人点。候选：crewd 用稳定签名身份并在建条目时把它加进信任列表；或 macOS 也改用 0600 文件；或由已签名的 Mac app 读出后经本机 socket 交给 crewd（与 §13 Q7「客户端只写不读」相冲突，需人定）。
- **实测集的代表性**：仓库公开、只能用合成用例；要更可信的阈值，需要一个不进仓库的本机私有用例集（同一格式，`--cases` 指过去），是否做、谁来标注，由人定。
- **new_crew 与「大厅消息归群」的执行**（§7.2、§17）：M5a 只给判断，执行流程在 M5b 定。
