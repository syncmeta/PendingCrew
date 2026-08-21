# PendingCrew 架构导览

> 这份文档回答一个问题：**「我想改 X，该去哪个文件？」**
>
> 写作纪律：每一条结论都从当前 `main` 的文件里核出来，尽量带上文件路径（必要时带行号）。
> 核不动的地方写「未核实」，不用「一般来说」填空。
> 与 `docs/internal/` 不同，**这份是活的**，改了结构请顺手改它。
>
> 核对基准：`main` @ `4dc855a`（2026-08-21）。

---

## 0. 三十秒版本

PendingCrew 是一个 **macOS app（同时能编出 iOS/iPad 产物）**，它做的事是：
在你自己的机器上把 `claude` / `codex` 这两个 CLI 当**子进程**拉起来，让它们围着一块
共享的**群聊白板**协作。白板是磁盘上的一堆 JSON 文件；agent 通过一个 **MCP server**
读写它，而那个 MCP server 就是 **app 自己的二进制换个 argv 再跑一遍**。

所以整个产品的核心机制是一个闭环，闭环的介质是文件系统：

```
   人 ──▶ app（GUI）───spawn PTY / stdio───▶ claude / codex 子进程
            │  ▲                                    │
            │  │                                    │ MCP 工具调用 / hook
   写命令 & │  │ 目录监听（DispatchSource）           ▼
   镜像文件 │  │ 250ms 合流                    同一个 app 二进制
            ▼  │                             （--mcp-serve / --mcp-hook）
        ~/Library/Application Support/PendingCrew/whiteboards/*.json
                        ▲                            │
                        └────────── 读写白板 ─────────┘
```

**没有服务器、没有账号、没有数据库。** 云端那半（Supabase 登录、跨设备遥控）代码在
仓库里，但后端坐标是占位值（`Sources/Services/CrewHostedConfig.swift`），开箱不通。
代价比听上去大：**实际编译出来的 33 个第三方模块里有 25 个（76%）只服务那条关掉的路径**——
见第 3 节。

规模：`Sources/` + `Shared/` 共 **249 个 Swift 文件 / 48,038 行**（`find … | wc -l`，
含 `#if` 屏蔽掉的行）；`Tests/PendingCrewTests/` **123 个文件 / 21,532 行 / 1,443 个
`func test`**。

---

## 1. 工程定义与构建

### 1.1 `project.yml` 是唯一真值

工程用 [XcodeGen](https://github.com/yonaskolb/XcodeGen) 生成。
`PendingCrew.xcodeproj/project.pbxproj` 被 git 跟踪，但**它是生成物**——跟踪它只是为了
让没装 XcodeGen 的人也能直接开工程。改了 `project.yml`（**包括新增 Swift 文件**）要重跑
`xcodegen` 并提交 `.pbxproj`，否则别人 clone 下来编不过（`CONTRIBUTING.md:22-33`）。

两个 target：

| target | 类型 | 平台 | 源 |
|---|---|---|---|
| `PendingCrew` | application | iOS + macOS（`supportedDestinations`） | 整个 `Sources/` + `Resources/` + `Shared/AppUpdate` |
| `PendingCrewTests` | bundle.unit-test | **仅 macOS** | `Tests/PendingCrewTests/` + 从 `Sources/` **逐文件挑进来**的 86 个 `.swift` + 3 个整目录（`Sources/Mac/LocalRunner`、`Sources/Models`、`Resources/Prompts`） |

- deployment target：iOS 17.0 / macOS 14.0（`project.yml:5-6`）
- Swift 语言版本：`SWIFT_VERSION: "5.0"`（`project.yml`，`settings.base`）
- `TARGETED_DEVICE_FAMILY: "1,2"`（iPhone + iPad）
- 版本号 `MARKETING_VERSION` 写在 `project.yml` 里，发版脚本用 `sed` 从这里读

### 1.2 一个 target = 一个 Swift 模块（重要）

`Sources/` 下那十几个目录**只是目录**。它们全部编进同一个 module，没有 `import`
边界、没有编译器强制的依赖方向。第 5 节讲的「分层」全靠约定和 code review 维持，
不靠工具。

### 1.3 测试 target 为什么要逐文件挑源码

`PendingCrewTests` 不是 host-app 注入式测试（`TEST_HOST: ""` / `BUNDLE_LOADER: ""`），
而是一个**独立 bundle**，把被测源码**直接编进来**。`project.yml` 里那一长串
`- path: Sources/...` 就是这个清单，每一条都带注释说明为什么它必须在。

代价是明确的：**一个类型能不能被单测，取决于它的依赖闭包能不能不带 SwiftUI/AppKit/
Supabase 编出来。** 这条约束反过来塑造了代码结构 —— 仓库里大量「纯判定逻辑抽成
自包含 Foundation 文件」的写法（`Sources/Support/` 几乎整个目录、`Sources/Stores/`
的大半）就是为了满足它。想加单测却发现编不进 bundle 时，正确的动作通常是把纯逻辑
切出来，而不是给 bundle 加依赖。

### 1.4 签名：仓库默认 ad-hoc

签名策略**不在 `project.yml` 的 `settings` 里**，在 `Config/Signing.xcconfig`（project 级
`configFiles`，Debug/Release 都指它）。原因写在 `project.yml` 的注释里：Xcode 的优先级是
「target 设置 > target xcconfig > project 设置 > project xcconfig」，只要
`DEVELOPMENT_TEAM` 还留在 `settings.base`，本地覆盖就永远不生效。

**为什么在 `Config/` 而不是仓库根**（2026-08-21，`4dc855a`）：根目录的 xcconfig 会让
`xcodegen` 的输出不确定 —— `project.pbxproj` 里那条文件引用的 uuid 每次 regen 都变，
于是 CI 那道「regen 之后 `git diff --exit-code`」永远是红的。挪进 `Config/` 之后就稳定了。

- **仓库默认**（`Config/Signing.xcconfig:28-42`）：`CODE_SIGN_STYLE = Manual`、
  `CODE_SIGN_IDENTITY = -`（ad-hoc）、`DEVELOPMENT_TEAM` 空、`CODE_SIGN_ENTITLEMENTS` 空。
  任何人 clone 下来不改被跟踪的文件就能编能跑。
- **本机覆盖**：`Config/Signing.xcconfig` 末尾 `#include? "Config/Local.xcconfig"`（gitignored，
  模板见 `Config/Local.xcconfig.example`），填自己的 Team ID 并打开 entitlements。
- **entitlements 必须跟着签名身份一起走**：`Resources/PendingCrew.entitlements` 里的
  keychain 组写成 `$(AppIdentifierPrefix)com.pendingname.pendingcrew`，这个变量**只能由
  provisioning profile 展开**，没 team 就直接编译失败（不是警告）。所以默认构建不带
  entitlements。
- **代价**：ad-hoc 每次重建签名身份就变 → `KeychainStore`（`Sources/Support/KeychainStore.swift`）
  的 ACL 认不出 → 云端登录态静默存不住。已登记为 🟡（`docs/tech-debt.md`）。
  **只跑本机 crew 完全不受影响 —— 那条路径一次都不碰钥匙串。**
- **发版不读这份文件**：`scripts/release/build-macos-update.sh` 在 xcodebuild 命令行上
  显式传 `CODE_SIGN_STYLE=Automatic` / `DEVELOPMENT_TEAM` / `CODE_SIGN_ENTITLEMENTS`，
  命令行优先级最高。

### 1.5 一个二进制，三副身份

`Sources/PendingCrewEntry.swift` 是 `@main`。它先看 argv：

| argv | 身份 | 干什么 |
|---|---|---|
| `--mcp-serve` / `--mcp-hook` / `--mcp-permission-hook` / `--mcp-turn-hook` | **crew-comms helper** | 不起 GUI，当 MCP server / claude hook 跑完即退（`Sources/Mcp/McpHelperMain.swift`） |
| `--render-snapshot <dir>` | dev 工具 | headless 用 `ImageRenderer` 出 codex transcript 的 light/dark PNG |
| 其余 | **GUI** | `PendingCrewApp.main()` |

在 helper 分支**之前**先做两件事（`PendingCrewEntry.swift:20-24`）：装未捕获 NSException
留痕（`UncaughtExceptionLog`）、把 fd 软上限抬到硬上限（`FileDescriptorLimit`）。
后者不是优化 —— launchd 给 GUI app 的默认软上限只有 256，而白板目录有上千个文件，
顶穿之后 `open()` 返回 EMFILE、被 Foundation 包成「你没有权限查看此文件」，2026-08-12
的数据事故就是这么来的。helper 子进程压的是同一批文件，所以也要抬。

「app 二进制兼当 MCP server」这个选择是有意的：比 embed 一个独立 executable 更自包含
（`Bundle.main.executablePath` 铁定可寻），也避开 macOS app bundle 嵌可执行文件的
签名/拷贝坑。**第三副身份 `--daemon` 已经在设计里**，见第 10 节。

---

### 1.6 这个仓库的 git 历史只回溯到 2026-08-15

第一个提交是 `a61e7a5 2026-08-15 initial import from monorepo snapshot 61d876a7`
—— PendingCrew 是那天从 PendingBot monorepo 的一份快照**压平重建**出来的独立仓库，
不是从 monorepo 里 filter-branch 切出来的。

**后果，查历史之前先知道这一条：**

- **2026-08-15 之前的任何 commit sha 在这里一律解析不出来**，`git cat-file -e` 会
  `fatal: Not a valid object name`。PendingBot 那边同期也重建过，所以那些 sha 在
  **两个仓库里都不存在**。这是设计使然，不是仓库损坏，也不是有人 force-push 掉了。
- **代码本身没丢** —— 快照带着当时的工作树进来了。8-15 之前的改动，**文件在树上、
  mtime 保留着原时间**，只是没有对应的提交记录。想知道某段代码什么时候写的，看
  `stat -f %Sm`，别看 `git log`。
- 只存在于「某次提交的 diff」里而没落在工作树上的东西（被删掉的文件、提交信息里的
  设计说明）**是真的没了**。

**这个坑的典型踩法**（2026-08-20 实际发生过一次，所以写在这儿）：拿一批旧 sha 去
`git cat-file`，全部解析失败，于是下结论「那批工作丢了」。**解析不出来只证明历史被
重建过，不证明工作不存在。** 判断一件事做没做，去树上找那件事的产物——文件、测试、
`project.yml` 里的挂载——别只查提交。

build 号也是因为这条才不再从 `git rev-list --count HEAD` 算的，理由见
`scripts/release/build-macos-update.sh` 顶部那段注释：历史是这个仓库里最脆的东西。

---

## 2. 七个 SPM 依赖

直接依赖 7 个（`project.yml` 的 `packages:`），解析出来一共 **23 个 pin**
（`PendingCrew.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved`）。
换句话说 **16 个是传递依赖**，其中 13 个只为云端登录那条路服务。
把两个直接依赖（GoogleSignIn、Supabase）也算进去就是 **15 / 23**——**这条单独立在第 3 节**，
它是整份梳理里最有决策价值的一条。

| 包 | 版本约束 | 实际解析 | 用在哪（唯一入口） | 换掉的代价 |
|---|---|---|---|---|
| **SwiftTerm** | `from: 1.13.0` | 1.18.0 | `Sources/Mac/LocalRunner/`：`AgentSessionCore` / `TerminalMirrorView` / `AgentTerminalSession` / `PlainTerminalSession`、`Sources/Mac/Views/AgentTerminalView.swift` | **形态性，换不掉。** 见 2.1 |
| **Sparkle** | `exactVersion: 2.9.2` | 2.9.2 | `Shared/AppUpdate/AppUpdater.swift`（**唯一** import） | **形态性，成本在分发侧不在代码侧。** 见 2.2 |
| **Supabase** | 2.0.0 ≤ v < 3.0.0 | 2.55.1 | `Sources/Auth/CrewEmailSignIn` / `CrewAppleSignIn` / `CrewGoogleSignIn`、`Sources/Services/CrewSupabaseStack.swift` | 低——只在云端登录路径，且只用来换一次性 session；本机 crew 一次都不碰。带 6 个传递依赖 |
| **GoogleSignIn** | 8.0.0 ≤ v < 9.0.0 | 8.0.0 | `Sources/Auth/CrewGoogleSignIn.swift`（**唯一** import，137 行） | 低——一个登录按钮。但它是全仓最贵的依赖：**独自拖进 7 个传递包** |
| **MarkdownUI** | 2.4.0 ≤ v < 3.0.0 | 2.4.1 | `Sources/Chat/Vendored/MarkdownText.swift` / `MathRendering.swift` | 中——群聊气泡的正文渲染全靠它；换等于重写 `MarkdownText`。带 2 个传递依赖 |
| **SwiftMath** | 1.0.0 ≤ v < 2.0.0 | 1.7.3 | `Sources/Chat/Vendored/MathRendering.swift`（**唯一** import） | 低——只渲染消息里的 LaTeX 公式，去掉等于降级成纯文本 |
| **TOMLKit** | 0.5.0 ≤ v < 1.0.0 | 0.6.0 | `Sources/Mac/LocalRunner/WorkspaceSync/WorkspaceManifest.swift`、`WorkdirMigrationExecutor.swift` | 低——读写 workspace manifest 与 `.codex/config.toml` |

传递依赖归属（各包 `Package.swift` + 解析图核对）：

- **GoogleSignIn → 7 个**：AppAuth-iOS 1.7.6、GTMAppAuth 4.1.1、gtm-session-fetcher 3.5.0、
  GoogleUtilities 8.1.2、app-check 11.3.1、promises 2.4.1、interop-ios-for-google-sdks 101.0.0
- **Supabase → 6 个**：swift-crypto 4.5.1（→ swift-asn1 1.7.1）、swift-http-types 1.6.0、
  swift-clocks 1.1.0、swift-concurrency-extras 1.4.1、xctest-dynamic-overlay 1.11.0
- **MarkdownUI → 2 个**：NetworkImage 6.0.1、swift-cmark 0.8.0
- **SwiftTerm → 1 个**：swift-argument-parser 1.8.2
- Sparkle / SwiftMath / TOMLKit：0 个

> 上面这四行按「谁把它拖进来的」数，得到 **13 个传递依赖**只服务云端登录；
> 连两个直接依赖一起数是 **15 / 23**，按实际编译出来的模块数则是 **25 / 33**。
> 三种数法与限定条件见 **第 3 节**。

### 2.1 SwiftTerm 为什么换不掉

它不是「一个终端控件库」，它是**这个产品的形态本身**：每个 session 是一个真 PTY，
用户可以直接接管键盘。SwiftTerm 提供的三样东西，缺一个产品就不成立：

1. `LocalProcess` —— PTY 属主（fork + setsid + winsize）
2. `Terminal` / `Buffer` —— **无画面**的终端状态机与权威缓冲区（纯 Foundation，零 AppKit）
3. `MacTerminalView` —— 原生渲染 + 选中复制 + 回滚 + 改宽度 reflow

而且第 2 与第 3 的分离正是「常驻后台」那条地基路线能走通的前提（第 10 节）：后台进程
养 1+2，窗口里的 view 只吃字节流。`TerminalView.feed(byteArray:)` 是 public 的，这是
库设计上就支持的用法。换库 = 同时重写 PTY 层、状态机、渲染层，并重新验证那**六个从终端
画面上认状态的扫描器 + 拉起自检看门狗**（`AgentTerminalSession.swift:12` 的原话）——
健康（未登录/撞额度）、待决策菜单、打字指纹、启动参数回显、切档回显都是从字节流里认的。

顺带一条踩过的坑：Sparkle 是本 app **第一个嵌入式二进制框架**（其余 SPM 依赖都编进
主二进制了），所以 `LD_RUNPATH_SEARCH_PATHS[sdk=macosx*]` 必须显式设成
`@executable_path/../Frameworks`（`project.yml`）——XcodeGen 对多端 target 只生成 iOS
约定的 `@executable_path/Frameworks`，那在 macOS 上解析到 `Contents/MacOS/Frameworks`，
**编译链接全过、一启动就 dyld SIGABRT**。发版脚本因此专门有一道
`scripts/release/verify-rpath-resolvable.sh`。

### 2.2 Sparkle 为什么也换不掉

不是代码耦合（唯一 import 在 `Shared/AppUpdate/AppUpdater.swift`，58 行），而是**已发布
的产物契约**：

- `Info.plist` 里钉死了 feed 地址 `https://updates.pendingname.com/pendingcrew/appcast.xml`
  与 EdDSA 公钥 `SUPublicEDKey`
- 线上每个已安装的 app 都在按这个 feed + 这把公钥验签更新
- 发版脚本用 Sparkle 自带的 `generate_appcast` 签 feed（`build-macos-update.sh`）

换更新框架 = 已装机用户全部收不到更新，只能靠人重新下载。这是分发侧的不可逆性，
不是代码难度。

接线细节：`AppUpdater` 自动**检查**开、自动**安装**关；`Info.plist` 缺 feed 或公钥时
`isConfigured == false`，Sparkle 完全不启动（不带病启动、不假装能更新）。
`UpdateCheckGate`（`Shared/AppUpdate/UpdateCheckGate.swift`）是一条纯逻辑闸：
**用户手点的检查永远放行，后台定时检查在「有 session 在跑」时拦下**，忙判定由
`SessionHost` 注入（`Sources/Mac/Services/SessionHost.swift:70-73`）。

---

## 3. 三分之二的依赖树，服务的是一条被关掉的路径

这条单独立一节，因为它是整份梳理里**最有决策价值的一条**，埋在上面的清单里会被漏掉。

**三种数法，同一个结论。**

| 数什么 | 云端登录那两族（Google + Supabase） | 全部 | 占比 |
|---|---|---|---|
| `Package.resolved` 的 pin | **15**（Google 家族 8 + Supabase 家族 7） | 23 | **65%** |
| 实际编译出来的模块 | **25**（Google 族 11 + Supabase 族 14） | 33 | **76%** |
| 编译产物体积（Debug `.o`） | 19.7 MB（Google 2.9 + Supabase 16.8） | 36.6 MB | 54% |

模块与体积是**实测**的：一次完整 macOS Debug 构建后数 `DerivedData/Build/Products/Debug/*.o`，
每个模块归到哪个包由各包 `Package.swift` 核对。逐个列出来：

- **Google / OAuth 族（11 个模块）**：`GoogleSignIn`、`AppCheckCore`、`AppAuth`、`AppAuthCore`、
  `GTMAppAuth`、`GTMSessionFetcherCore`、`GoogleUtilities-{Environment,Logger,UserDefaults}`、
  `third-party-IsAppEncrypted`、`FBLPromises`
- **Supabase 族（14 个模块）**：`Supabase`、`Auth`、`PostgREST`、`Realtime`、`RealtimeV2`、
  `Storage`、`Functions`、`Helpers`、`Crypto`、`HTTPTypes`、`Clocks`、`ConcurrencyExtras`、
  `IssueReporting`、`XCTestDynamicOverlay`
- **本机 crew 主路径实际用到的只有 8 个模块**：`SwiftTerm`、`MarkdownUI` + `NetworkImage` +
  `cmark-gfm` + `cmark-gfm-extensions`、`SwiftMath`、`TOMLKit` + `CTOML`
  （外加嵌入式的 `Sparkle.framework`）

**为什么这条要紧**：README 的「状态」一节把云端登录归进「代码在，但这份开源仓库里开箱
不可用」——判据是 `Sources/Services/CrewHostedConfig.swift` 的四个占位常量。

而这两族的**入口窄得可以数清**。全仓 `import` 普查（`find Sources Shared -name '*.swift'
-exec grep -hoE "^ *import +[A-Za-z0-9_]+" {} +`）：`import Supabase` **4 处**（`Sources/Auth/`
的三个登录方式 + `Sources/Services/CrewSupabaseStack.swift`），`import GoogleSignIn` **1 处**
（`Sources/Auth/CrewGoogleSignIn.swift`）。**其余 23 个模块没有任何一处被源码直接 import**——
它们全部是这两个入口的传递依赖。

所以：**任何人 clone 下来编出的这个 app，四分之三的第三方模块的代码路径一次都不会被走到**
（链接期开销与 ObjC 运行时的加载开销仍在，那是另一回事）。一个「装上就能用、不需要登录、
不需要后端」的本机工具，它的依赖树主体是给一条默认关闭的云端路径准备的。

**别把它读成「砍掉就瘦一半」——三个必须一起说的限定**：

1. **数量 76%、体积只有 54%**。SwiftTerm 一个模块就 7.4 MB（Debug `.o`，全仓最大），
   MarkdownUI 3.9 MB、SwiftMath 2.2 MB。真正贵的那几个恰恰在主路径上。
2. **Debug `.o` 不等于发布产物的体积**。Release 会 strip、会做死代码消除，静态链接下没被
   引用的符号很大一部分根本不会进最终二进制。**上表的体积列只能当量级信号，不是分发体积。**
   要真数得先量一次 Release 产物，本次没量。
3. **这不是「有人乱加依赖」**。这条线是有意的产品边界（README：「本地必须自洽，登录只是
   叠加」），只是边界画在了源码里、没有画在依赖图里。

**真要动的话，代价从低到高**：把 `Sources/Auth/` 那几个登录方式做成可选（`GoogleSignIn`
带 7 个传递包，去掉它一族就少 8 个 pin / 11 个模块）＞ 把云端那半整体拆成 SPM 可选
product ＞ 拆 target。前两条都动 `project.yml` 的依赖声明，**属于结构改动，不要顺手做**。

## 4. 三端共用一套源码

同一个 target 同时编 macOS 和 iOS。**平台门是逐文件的 `#if os(macOS)`，不是逐目录的。**

| 目录 | 文件数 | 含 `#if os(macOS)` 的文件数 |
|---|---|---|
| `Sources/Mac/` | 110 | 99 |
| `Sources/Chat/` | 42 | 9 |
| `Sources/Stores/` | 28 | 6 |
| `Sources/Support/` | 20 | 0 |
| `Sources/Views/` | 10 | 3 |
| `Sources/Services/` | 10 | 2 |
| `Sources/Models/` | 8 | 1 |
| `Sources/Mcp/` | 7 | 0 |
| `Sources/Auth/` | 5 | 0 |
| `Sources/Remote/` | 3 | 0 |
| `Shared/AppUpdate/` | 4 | 2 |

⚠️ **`Sources/Mac/` 里有 11 个文件根本不是 macOS 专有的**，其中几个还是三端主路径上的
关键文件：

```
Sources/Mac/LocalRunner/AgentQuota.swift            ← Mcp 的 get_quota 要用
Sources/Mac/LocalRunner/AgentModelCatalog.swift     ← Mcp 的 start_session / set_session_profile 要用
Sources/Mac/LocalRunner/AgentModelProbeParsers.swift
Sources/Mac/LocalRunner/TypingActivityTracker.swift
Sources/Mac/LocalRunner/SessionStopCoordinator.swift
Sources/Mac/LocalRunner/QuotaWarningPlan.swift
Sources/Mac/Views/Chat/CrewSenderResolver.swift     ← Chat/Adapter 要用
Sources/Mac/Views/Chat/CrewTimeSeparator.swift
Sources/Mac/Views/Chat/CrewInteractionCard.swift
Sources/Mac/Views/Chat/CrewMentionPicker.swift
Sources/Mac/Views/Chat/CrewRosterBar.swift
```

另外 `Sources/Mac/Views/CrewChatView.swift`（1,437 行）**两端都编** —— 它就是
iPad/iPhone 上的群聊页（`Sources/Views/IPadShell.swift:47` 直接构造它），只是内部按
`#if os(macOS)` 分叉。已登记为 🟢（`docs/tech-debt.md`）。

**改动纪律**：三端都编一遍。只编 Mac 会让漏了 `#if os(macOS)` 的 AppKit 调用把 iOS 端
静默打红（`CONTRIBUTING.md:35-49`）。

---

## 5. 目录分层与真实依赖方向

### 4.1 各目录的职责

| 目录 | 行数 | 职责 |
|---|---|---|
| `Sources/Mac/` | 25,263 | macOS 主体。`LocalRunner/`（agent 子进程 + 终端内核 + workspace 同步）、`Services/`（长期服务）、`Views/`（三栏界面）、`Support/`、`Design/` |
| `Sources/Chat/` | 6,136 | 群聊 UI。`Vendored/`（从 PendingBot 逐字拷来的气泡/Markdown/composer）、`Adapter/`（白板 → 气泡的映射与纯逻辑）、`Shims/`（替掉 vendored 代码里 PendingCrew 没有的后端） |
| `Sources/Stores/` | 6,003 | 本地持久化 + app 级状态（`AppModel` / `CrewStore`） |
| `Sources/Mcp/` | 2,166 | crew-comms MCP server、三个 claude hook、未读游标、回合留痕 |
| `Sources/Services/` | 2,178 | 后端抽象（`PendingCrewBackend` 协议 + Local/Edge 两个实现）、edge REST 客户端、Supabase 栈 |
| `Sources/Views/` | 1,847 | 跨端 / iOS 侧界面（欢迎页、crew 列表、iPad shell） |
| `Sources/Support/` | 1,499 | 纯判定逻辑与小工具（几乎全部可单测、几乎全部零平台门） |
| `Sources/Models/` | 1,070 | 值类型（`CrewSummary` / `CrewDetail` / 驾驶舱模型） |
| `Sources/Auth/` | 877 | 云端登录（邮箱验证码 / Apple / Google / Turnstile） |
| `Sources/Remote/` | 634 | 跨设备遥控的 WS 协议编解码 + 客户端（**未接通**，见 README「状态」） |
| `Shared/AppUpdate/` | 121 | Sparkle 封装 + 构建版本戳（与 PendingBot 共用同一份设计） |

### 4.2 真实的依赖方向

用「A 目录的代码里出现了 B 目录声明的类型名」统计（剥掉注释，排除 `CodingKeys`
这类各处重名的嵌套类型）。主干是清楚的：

```
                 Sources/Views  Sources/Auth
                       │             │
                       ▼             ▼
   Sources/Mac ──────────────▶ Sources/Chat ──▶ Sources/Support
        │  │                        │                 │
        │  └──▶ Sources/Mcp ────────┤                 │
        │              │            ▼                 ▼
        └──────────────┴──▶ Sources/Stores ──▶ Sources/Models
                                    │
                                    ▼
                            Sources/Services
```

方向大体是「界面 → 编排 → 存储 → 模型」，**但它不是无环的**，且有三处值得知道的
反向引用：

1. **`Sources/Mcp` → `Sources/Mac`（真反向，但是有意的）**
   `Sources/Mcp/McpServer.swift` 的 `get_quota` / `start_session` / `set_session_profile`
   用了 `AgentQuotaFile`（`Sources/Mac/LocalRunner/AgentQuota.swift:640` 引用处）与
   `AgentModelCatalog` 一族（`McpServer.swift:1043-1083`）。这两个文件是**纯 Foundation、
   不带平台门**的，文件头注释明说了「McpServer（跨平台编译）要用」。所以这是**放错了
   目录**，不是真的跨层依赖。

2. **`Sources/Support` → `Sources/Mac`（同上）**
   `Sources/Support/QuotaRingLayout.swift` 用 `AgentQuotaSnapshot` / `AgentQuotaWindow`。

3. **`Sources/Stores` → `Sources/Mac`（真跨层）**
   `Sources/Stores/WorkspaceSyncStore.swift` 直接调 `SyncEngine` / `WorkspaceRepoService` /
   `WorkspaceRepoLayout` / `MachineRegistration`（都在 `Sources/Mac/LocalRunner/WorkspaceSync/`，
   且都带 `#if os(macOS)`）；`Sources/Stores/LocalCrewStore.swift:141` 用
   `WorkdirMigrationPlan`。这一条是 store 层反过来驱动 macOS 服务层，是三处里最实的。

`Sources/Chat` → `Sources/Mac` 只有两处（`CrewSenderResolver` / `CrewRemoteImage`），
其中前者也是「放错目录的跨平台文件」。

**为什么没被编译器拦下**：见 1.2 —— 全仓一个 module，不存在编译期的层。

### 4.3 有 22 个文件是从 PendingBot 拷来的，不是抽象出来的

`grep -rln "^// VENDORED" Sources/` → **22 个文件**，分布在两处：
`Sources/Chat/Vendored/`（18 个文件里的 17 个）与 **`Sources/Auth/`（全部 5 个文件）**。
另有 3 个 `// SHIM` 头在 `Sources/Chat/Shims/`。规矩写在文件第一行：

```
// VENDORED from PendingBot apps/pendingbot/Sources/Features/Message/BubbleView.swift @ 43c8ea2e
// 再对齐 = 对照源文件重拷。仅允许的偏离打 `// PENDINGCREW SHIM:` 标注。
```

改这些文件之前先看那行注释 —— 上游是另一个（未开源的）仓库，随手改会让下次对齐变成
手工三方合并。`Shims/` 里的 `ServerImage` / `UserAvatar` / `AttachmentDownload`
存在的唯一目的，是让 vendored 代码里对 PendingBot 后端的调用**签名不变地**接到
PendingCrew 自己的实现上（比如 `ServerImage` 的 `serverURL` 参数收下但忽略）。

`Sources/Auth/` 整个目录都是这类拷贝（Turnstile / 验证码状态机 / SIWA / Google），
偏离处逐条写在各文件头注释里。这也解释了为什么它是全仓最"不像本项目"的一块。

---

## 6. 进程模型：app ↔ 子进程 ↔ MCP

这是整个产品的核心机制，也是最值得先读懂的一节。

### 5.1 起一个 session（claude 那条腿）

编排入口 `Sources/Mac/Services/CrewSessionRunner.swift`（2,081 行，全仓最大的文件），
共享准备工序在 `Sources/Mac/LocalRunner/LocalSessionLaunch.swift`。

**第一步：找到 CLI。** `Sources/Mac/LocalRunner/LocalCodingAgentExecutable.swift`。
GUI app 从 launchd 拿到的是短 PATH（通常只有 `/usr/bin:/bin:/usr/sbin:/sbin`），
所以这里**起一次 `$SHELL -lic` 把用户真实 PATH 抓回来**（进程级缓存），再叠一组常见安装
目录兜底。同一条 PATH 也传给子进程（`childProcessPath`）—— 「定位用富 PATH、运行用短
PATH」那个不对称踩过：npm 装的 `codex` 是 `#!/usr/bin/env node` 脚本，PATH 里没 node
就 shebang 执行失败、进程秒退。

**第二步：拼 argv。** `Sources/Mac/LocalRunner/SessionConfig.swift` 的 `argv()`：

```
claude  --resume <id> | --session-id <uuid>
        --permission-mode auto            ← 是 auto mode，不是 --dangerously-skip-permissions
        --append-system-prompt-file <世界观.md>
        --settings <settings.json>        ← 三个 hook
        --mcp-config <mcp-config.json>    ← crew MCP server
        --model <m> --effort <e>
        -- <首条指令>                      ← `--` 必须有：--mcp-config 是变参，会吞掉 prompt
codex   app-server                        ← 只有这一个 token，其余全走协议
terminal <shell> -l
```

**第三步：写两个临时配置文件**（`LocalSessionLaunch.prepareLocalCommsConfig`，
落在 `FileManager.default.temporaryDirectory`）：

- `pendingcrew-mcp-<sessionId>.json` → `{"mcpServers": {"crew": {"command": <app 自己的可执行文件>,
  "args": ["--mcp-serve", "--crew", …, "--dir", <白板目录>, "--session", …, "--agent", "claude"]}}}`
- `pendingcrew-settings-<sessionId>.json` → 三个 hook，command 都指向同一个二进制：

  | hook | argv | 作用 |
  |---|---|---|
  | `PostToolUse` | `--mcp-hook` | 每轮把**本 session 未读的白板**注进上下文（`Sources/Mcp/HookEmitter.swift`） |
  | `PreToolUse` | `--mcp-permission-hook --gate computer-use` | 命中 gate 的工具 → raise 待审批 + 阻塞 long-poll 等人拍板 |
  | `Stop` | `--mcp-turn-hook` | 这一轮它一次都没往群里说过话 → 系统替它把收尾话头发进群 |

  注意 v1 只 gate `computer-use`，**不** gate crew 自己的工具（它们都是 `mcp__crew__` 前缀，
  子串不含 `computer-use`）。

**第四步：世界观。** `Sources/Stores/LocalSessionWorldModel.swift` +
`Resources/Prompts/session-world-model.{zh,en}.md` + `crew-captain.zh.md`（机长 persona），
用 `PromptTemplate`（纯 `{{var}}` 替换）渲染成临时 `.md`，claude 走
`--append-system-prompt-file`（**append，不 replace**）。里面填的是：本 crew 是什么、
群里有谁、机长是谁、工作目录、组织树上下位置、两家 runner 的订阅档位。

**第五步：拉起。** `AgentSessionCore`（`Sources/Mac/LocalRunner/AgentSessionCore.swift`，
647 行）持有 SwiftTerm 的 `LocalProcess` + 无画面 `Terminal`，回滚 10,000 行。
`TerminalMirrorView` 只负责画。停进程走 `terminateTree`（SIGTERM → 宽限 → `killpg`
整组 SIGKILL），因为 PTY 子进程是 setsid 的会话首，杀组才能连带杀掉它拉起的
bash / MCP 子进程。

### 5.2 codex 那条腿不一样

codex 不跑交互式 TUI，跑 `codex app-server`（stdio JSON-RPC），
`Sources/Mac/LocalRunner/CodexAppServer/`：

| 文件 | 职责 |
|---|---|
| `CodexAppServerConnection.swift` | 进程 + 三根管道；`actor`。**stderr 也必须排空** —— 不读的话 64KB 内核管道满、codex 阻塞在 write，表现为「turn 永远不完成」 |
| `CodexRPCMessage.swift` / `CodexRPCDispatcher.swift` | 分帧、双 id 路由、早到应答缓冲 |
| `CodexProtocol.swift` | `thread/start` / `thread/resume` / `turn/start` 的参数构造 |
| `CodexAppServerBackend.swift` | 实现 `SessionBackend`，让上层编排看不出两条腿的差别 |
| `CodexTranscript.swift` / `CodexThreadItem.swift` | 结构化 transcript（不是终端画面） |

同一套 crew 能力经协议注入：MCP server 走 `thread/start` 的 `mcp_servers`（`LocalSessionLaunch.codexMcpServers`），
世界观走 `developerInstructions`，每轮的未读白板走 codex 原生的
`turn/start.additionalContext`（`{"crew_whiteboard": {"value": …, "kind": "untrusted"}}`，
`CodexProtocol.swift:113`），**不塞进 `input` 冒充用户输入**。

两条腿的统一契约是 `Sources/Mac/LocalRunner/SessionBackend.swift`（132 行，值得完整读一遍）。
它把差异逐条写进注释，其中两条最关键：

- `isBusy`：codex 有真 turn 生命周期；**claude 恒返回 `false`**（交互式 claude 没有可编程
  的 turn-state，写进去的文本由它自己排队）。但这条只对**普通 prompt** 成立——斜杠命令
  （`/model`、`/effort`）忙时会被排成一句字面文本，所以 `applyProfileSwitch` 必须先等空闲。
- `pendingDecision`：claude 从**终端画面**认出选择菜单；codex 没有菜单，走结构化
  server-request，当场发群。

### 5.3 MCP server 提供了什么

`Sources/Mcp/McpServer.swift`（1,147 行）。newline-delimited JSON-RPC over stdio，
`handleLine` 是纯函数式 dispatch（一行进、一行出，不碰进程/stdio）——所以它能编进测试
bundle 单测。工具集（`tools/list` 在 `McpServer.swift:74-387`）：

- **通信**：`post_to_crew`（可带 `mentions` / `reply_to`）、`read_whiteboard`、`listen`
- **求助**：`ask`（raise 一条待决策 → 阻塞 long-poll → captain 或人类答复）
- **通讯录**：`directory`、`contact`（跨 crew 喊话，两边留痕）
- **自我管理**：`get_quota`、`schedule_wakeup`、`set_session_profile`、`respond_todo`
- **机长专用**（`--captain` 才解锁）：`answer_decision`、`start_session`、`inspect_session`、
  `nudge_session`、`stop_session`、`list_sessions`、`rename_crew`、`raise_attention` /
  `clear_attention`、`change_workdir`、`report_to_parent`、`message_child_crew`、
  `create_child_crew`、`create_parent_crew`、`adopt_crew` / `adopt_parent` / `release_crew`

### 5.4 helper 是离线的 —— 它怎么让 app 干活

**关键约束：helper 是个短命子进程，碰不到 app 的内存，也没有网络。它唯一能碰的是
`--dir` 下的共享文件。** 于是回路是双向的文件通道：

**helper → app（命令通道）**，`Sources/Stores/LocalCrewControlStore.swift`：

- 一次性元数据：`<crewId>.crewmeta.json`（改名，last-write-wins）、
  `<crewId>.crewattention.json`（黄点）
- 命令队列：每条一个独立文件 `<crewId>.<cmdId>.crewcmd.json`，app 侧 `CrewStore` 排空
  后写 `<crewId>.<cmdId>.crewresp.json` 回执。命令种类见 `LocalCrewControlStore.swift:129-321`：
  `start_session` / `create_child_crew` / `set_profile` / `crew_message` / `schedule_wakeup` /
  `listen` / `inspect_session` / `nudge_session` / `stop_session` / `change_workdir` /
  `adopt_crew` / `release_crew` / `create_parent_crew` / `adopt_parent`

**app → helper（镜像文件）** —— app 内存里的实时状态，helper 看不见，所以 app 定时落盘：

| 文件 | 谁写 | 节奏 | 谁读 |
|---|---|---|---|
| `crew-sessions.json` | `CrewSessionRunner.startSessionsSnapshotTimer`（`CrewSessionRunner.swift:616`） | **2 秒** | 机长的 `list_sessions` |
| `quota.json` | `QuotaCenter`（`QuotaCenter.swift:63`） | **600 秒** | `get_quota`、世界观渲染 |
| `models.json` | `ModelCatalogCenter`（`ModelCatalogCenter.swift:47`） | **6 小时** | `start_session` / `set_session_profile` 的模型表校验 |

### 5.5 app 怎么知道 helper 写了东西

`LocalWhiteboardStore.startWatching()` 在白板目录上挂一个 `DispatchSource` 目录监听，
**250ms 合流**（`DirectoryWatchCoalescing.swift`；目录里上千个文件持续在写，不合流会把
订阅方按写盘频率反复拉起）。这一个信号驱动全部下游：

1. UI 刷新（`LocalBackend.whiteboardChanges` → 群聊列表、审批卡片）
2. **`CrewLocalMentionWaker`**（`Sources/Mac/Services/CrewLocalMentionWaker.swift`）——
   把白板里 session/机长发的**定向 @** 转成对目标 run 的注入；目标完全没在跑就把它
   拉起来。没有这一环，机长派的活会全部躺在白板里没人醒（2026-07-19 的事故）。
3. `CrewStore` 排空控制通道的命令与改名

注入不重复靠 `Sources/Mcp/WhiteboardCursor.swift`：per-session 游标文件
`<crewId>.<sessionId>.cursor`，位置是 **(id, createdAt) 复合**而非裸 id，写入 forward-only
且过 flock —— 因为 hook 路（helper 进程）和唤醒路（app 进程）是**两个进程在写同一个游标**。
2026-08-12 全机重放事故的最后两环就在这里：白板被误判损坏重建 → 换了一批 id → 游标集体
悬空 → 「悬空」被当成「全是新的」→ 每次唤醒全量重放几周前的 @。现在的规矩是
**游标认不得绝不等于全是新的**（fail-closed）。

---

## 7. 数据与持久化

**全部在本机，没有数据库，没有上传。** 根目录 `~/Library/Application Support/PendingCrew/`。

```
PendingCrew/
├── local-crews.json                 crew 树（DAG）+ 全机 crew 号码簿
├── captain-templates.json           机长模板池（BYOK）
├── attachments/<crewId>/            群聊图片/文件（随聊天记录持久）
├── composer-staging/                托盘里还没发出去的附件
├── backups/
└── whiteboards/                     ← 唯一的跨进程共享面
    ├── <crewId>.json                白板消息 [LocalWhiteboardMessage]
    ├── <crewId>.approvals.json      待审批 / 待决策
    ├── <crewId>.todos.json          人类 Todo 面板
    ├── <crewId>.crewmeta.json       待改名（控制通道）
    ├── <crewId>.crewattention.json  黄点（控制通道）
    ├── <crewId>.<cmdId>.crewcmd.json / .crewresp.json   机长命令 + 回执
    ├── <crewId>.captain-awareness.json  命名/拆组提示的去重状态
    ├── <crewId>.<sessionId>.cursor  per-session 未读游标（id + 时间戳）
    ├── <crewId>.<sessionId>.turn    回合结束 marker
    ├── agent-sessions.json          agent 侧会话号（重启后 --resume 回原对话）
    ├── wakeups.json                 定时唤醒账
    ├── quota.json / models.json / crew-sessions.json   app→helper 镜像
    └── *.lock                       每个账本一把 flock sidecar
```

（本机实测：36 个 crew、`whiteboards/` 下 1,346 个文件。）

### 6.1 并发模型

`Sources/Stores/MultiProcessJSONStore.swift`（282 行）是共用基座，**四件套**：

1. **flock sidecar 互斥** —— read-modify-write 全程在锁内，消除 last-write-wins 丢写
2. **逐条 lenient 解码** —— 数组里坏一条丢一条，不连坐成整文件解码失败
3. **corrupt 归档 fail-loud** —— 外层 JSON **确认**解不开时归档成 `<file>.corrupt-<毫秒>`
   并回调调用方喊出来，**绝不** `(try? decode) ?? []` 把文件静默当空
4. **读失败 ≠ 内容损坏** —— 「文件读不出来」永远不许归档/搬走/删除/重建原件；只有
   **两次独立读到的字节都真解不开**才算损坏。读失败带三级退避重试（20/60/150ms）

第 4 条是血写的，注释里记着账：2026-08-12 晚上四轮误杀，19–24 份**完全合法**的 JSON
被归档并从空重建，约 2000+ 条群聊历史从 live 文件消失。病根不是解码，是 fd 打满
（见 1.5）。这个基座还把「读不出来」和「真解不开」分成两套文案
（`LedgerIncident`），因为当晚一半的无效轮次是那句假描述「文件损坏，已归档」造成的。

**走这个基座的**：白板、approvals、todos、wakeups、agent-sessions、控制通道的命令队列、
`CrewLastMessageCache`、`CrewSessionRunner` 的快照写入。

**不走的**（单进程单写者，只用 atomic write）：`local-crews.json`（`LocalCrewStore`，
`@MainActor`，helper 只读不写——它要改 crew 元数据得走控制通道）、`captain-templates.json`、
`quota.json` / `models.json` / `crew-sessions.json`（app 单写、helper 单读）。

### 6.2 性能门控

白板目录事件不带文件名，所以一个 tick 要把每个 watched crew 都扫一遍，而「扫」=
取文件锁 + 整份读 + 整份 JSON 解码。为此有一层文件指纹门控
（`Sources/Stores/FileFingerprintCache.swift`），三个消费方各有一个缓存：
`CrewLastMessageCache`（侧栏每个 crew 的末条消息）、`SessionAwaitingReplyInputsCache`
（点名快照要的审批账 + 回合 marker）、`CrewLocalMentionWaker` 自己那份。
实测收益写在 `docs/tech-debt.md` 与各文件注释里（9–11ms → 0.07–0.10ms 一档）。

---

## 8. 构建与发版

### 7.1 日常

```sh
xcodegen                                                        # 改了 project.yml 之后
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' build
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' test
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'generic/platform=iOS Simulator' build
```

每次构建后跑一个 post-build script：`Shared/scripts/stamp-build-info.sh`，把「这个产物
从哪个 commit 出的」写进产物 `Info.plist`（`basedOnDependencyAnalysis: false` —— 没有输入/
输出文件可声明，必须每次都跑，否则增量构建沿用上次的戳，戳就成了谎报）。

**CI 在 `.github/workflows/ci.yml`**（2026-08-21 落地）。两个 job：①「pbxproj 与
`project.yml` 同步」—— 重跑 `xcodegen` 后比 diff，抓「加了文件忘了 regen」那条；
② 三端编译 + 单测。在这之前上面三条命令全靠人手跑，而第一条的失败症状恰好是
**只有别人的机器编不过**，提交者本机永远看不到红。

### 7.2 发版：一条命令，六道断言

```sh
PENDING_NOTARY_PROFILE=pendingcrew-notary scripts/release/build-macos-update.sh
# 要顺带发到线上自动更新 feed 才加 PENDING_PUBLISH_R2=1
```

`scripts/release/build-macos-update.sh` 的流程：

```
git worktree 钉 main HEAD 建干净快照   ← 工作区脏不脏都不影响所见即所装
  ↓
build 号 = 纪元日时间戳「天.秒」（如 20684.16770），从时钟派生不从 git 历史算
  ↓
拉线上 appcast 比一次 —— 不严格大于线上最大值就拒绝构建
  三态：拉到且有版本→比大小 / 拉到但为空→合法首发放行 / 拉取失败→拒绝（fail-closed）
  ↓
xcodebuild archive（自动签名）→ exportArchive（method: developer-id，重新签发布身份）
  命令行显式传 CODE_SIGN_STYLE / DEVELOPMENT_TEAM / CODE_SIGN_ENTITLEMENTS
  （干净快照里没有 gitignored 的 Config/Local.xcconfig）
  ↓
六道断言，每道对应一次真踩过的坑：
  ① Info.plist 必须有 SUPublicEDKey（缺了 = 永远收不到更新的死包，两端都静默）
  ② 必须嵌进 embedded.provisionprofile（缺了 AMFI 在 exec 前拒，表现是「无法打开」）
  ③ 签完的 entitlements 不许残留未展开的 $(…) 构建变量
  ④ 必须真的带上 <team>.com.pendingname.shared（正面断言，不只是反面判据）
  ⑤ 源 entitlements 声明的键逐项在产物里核对（Xcode 会把未授权的键**静默剥掉**）
  ⑥ hardened runtime 必须开 + verify-rpath-resolvable.sh 把 dyld 解析规则跑一遍
  ↓
ditto → notarytool submit --wait → stapler staple → 重新 ditto
  ↓
generate-release-notes.sh → Sparkle 的 generate_appcast 签 feed
  ↓
codesign --verify / spctl -t install / stapler validate → git tag v<版本>
  ↓
（可选）publish-macos-update-r2.sh → wrangler 传 R2（updates.pendingname.com）
```

签名**全权交给 Xcode**（archive + exportArchive），脚本里不再出现任何一行手写 `codesign`
—— 2026-08-06 两次出血都源于手工复刻 Xcode 的签名行为却只复刻了看得见的那部分。

### 7.3 GitHub Release 是另一条路

**挂 Release 用手边那份已公证的产物，不为了发 Release 而构建新版本**
（`docs/release-macos.md:15-53`）。原因：`main` 上随时躺着没人工验收过的改动，为了挂
Release 把它们构建成新版本 = 把未验收的东西推给自更新用户（Sparkle 看的是 build 号）。

```sh
scripts/release/make-dmg.sh dist/updates/pendingcrew/PendingCrew-<版本>.zip
git push origin v<版本>
gh release create v<版本> --draft … <.dmg> <.zip>
```

- `.dmg` 是给人手动下载的主入口（`.zip` 直接在「下载」文件夹双击会踩 App Translocation）
- `.zip` 是 Sparkle 吃的那一份（Sparkle 只认 zip）
- `--draft` **不是可选的** —— 发布这一下是人的决定，不是脚本的
- 挂 Release **不开** `PENDING_PUBLISH_R2`，那是两条独立的分发路径

### 7.4 装到本机

发版脚本只产出包，不负责装。装那一步要等人把正在跑的 PendingCrew 退掉（所有 session
会随旧进程结束），所以走一个挂在 launchd 底下的安装器：与 PendingCrew 不同进程组、
等待窗口 90 分钟、**超时原样放弃一个文件都不动**。顺序是冷备份数据目录 → 旧 app 移到
回滚位 → 装新的 → 自动重开。细节与回滚命令见 `docs/release-macos.md:95-106`。

> 「更新 app 就得等所有 session 跑完」正是第 10 节那条地基路线要解掉的问题。

---

## 9. 测试

`Tests/PendingCrewTests/`：123 个文件、21,532 行、**1,443 个 `func test`**
（`grep -rh "func test" Tests/PendingCrewTests/*.swift | wc -l`）。

**本次实测**（`xcodebuild … -destination 'platform=macOS' test`，本机 2026-08-20）：
**Executed 1443 tests, with 3 tests skipped and 0 failures，184.5 秒，`** TEST SUCCEEDED **`**。
（跑得慢的是 `SyncEngine*Tests` / `ProjectSyncServiceTests` —— 它们真的建临时 git 仓库
做双机推拉，单个用例 6–10 秒。）

### 8.1 覆盖了什么

看得出明确意图的几块：

| 面 | 代表测试 | 钉的是什么 |
|---|---|---|
| MCP 协议面 | `McpServerTests` 及 7 个 `McpServer*Tests` | 每个工具的 dispatch、参数校验、机长权限门 |
| 白板与账本 | `LocalWhiteboardStoreTests`、`MultiProcessJSONStoreReadFailureTests` | flock、lenient 解码、**读失败绝不销毁原件** |
| 未读与唤醒 | `WhiteboardCursorFailClosedTests`、`CrewLocalMentionWake*Tests`（4 个文件） | 游标 fail-closed、不重放、不漏注入 |
| 两条 runner 腿 | `SessionConfigTests`、`CodexRPCDispatcherTests`、`CodexProtocolTests`、`AgentSessionCoreTests` | argv 构造、JSON-RPC 分帧与双 id 路由、终端内核 |
| 终端劈半的等价性 | `TerminalMirrorParityTests`、`TerminalScrollbackTests`、`TerminatedScrollbackTests` | P1 劈开后画面与内核不漂 |
| 界面自激回归 | `LayoutLoopRegressionTests`、`NoSwiftUIRepeatForeverTests` | 「布局自激闪退」不许回来 |
| 性能预算 | `CrewChatOpenCostTests`（8 个用例，真数据 + 离屏 `NSHostingView` 实测毫秒） | 打开群聊一次重排 ≤ 100ms |
| 组织与通讯录 | `CrewDirectoryTests`、`LocalCrewStoreOrgMoveTests`、`CaptainOrgToolsTests`、`CrewDragDropLogicTests` | 禁环、号码不回收、拖拽语义 |
| 泄密闸 | `CrewHostedConfigTests` | 钉住仓库里必须还是占位后端坐标 |
| 视图接线 | `ViewWiringTests`、`ProcessRoleTests` | 视图不许 new 长期对象；进程角色判定 |

### 8.2 没覆盖什么（结构性的，不是遗漏）

- **UI 本身**：没有 XCUITest，没有快照测试。SwiftUI 视图只以「离屏 `NSHostingView` 量
  布局成本」和「静态接线检查」的形式被间接触到。
- **真子进程**：没有任何测试真的拉起一次 `claude` / `codex`。argv、协议、解析器全被单测，
  但「拼出来的这条命令行在真 CLI 上跑不跑得通」只能靠人。
- **端到端回路**：「app 写 → helper 读 → agent 调工具 → 写回 → app 醒」这条链没有集成测试，
  各段单测都很扎实，接缝靠人。
- **iOS 端**：测试 bundle 只挂 macOS（被测代码基本都在 `#if os(macOS)` 后面）。
  iOS 只有「编得过」这一层保证。
- **发版脚本**：六道断言是运行时的，没有对脚本本身的测试。

### 8.3 那 11 个 skip 是怎么回事

**干净 clone 上会 skip 11 条**，来自三个不同的原因，别混成一件事：

| 数量 | 哪些 | 为什么 |
|---|---|---|
| **8** | `CrewChatOpenCostTests` 全部 8 个用例 | **缺不入 git 的 fixture** |
| 2 | `CrewLastMessageCacheTests.test_基准_现场白板目录`、`SessionAwaitingReplyInputsCacheTests.test_基准_现场目录` | 要用环境变量指一份现场白板目录才跑（`TEST_RUNNER_PENDINGCREW_BENCH_WHITEBOARD_DIR=…`） |
| 1 | `FamilyCredentialStoreTests.testSetGetClearRoundtrip` | ad-hoc / headless 构建下 keychain entitlement 不生效，SecItem 全部 `errSecMissingEntitlement` |

**那 8 条的 fixture 是什么**：`Tests/PendingCrewTests/Fixtures/LEDDriverCrew/` ——
一份**人类真实的群聊内容 + 真实聊天截图**（约 1MB），故意不进版本历史（`.gitignore:20-22`）。
自己取：

```sh
scripts/make-chat-fixtures.sh <crew-id>
ls "$HOME/Library/Application Support/PendingCrew/whiteboards"   # 挑一个 crew id
```

脚本默认只取**前 70 条**，因为整套预算（一次重排 100ms 等）是在那个 crew 还是 70 条 /
58,945 字节那天定标的；白板是 append-only，所以「前 70 条」就是当初那份快照本身。
用真数据不用造的，理由写在脚本注释里：**造出来的数据量不出真问题**——本 crew 338 条、
82k 字、也带图，实测比 LED 那 70 条还快，卡不卡取决于单条有多贵而不是条数。

这几条 2026-08-20 从「缺了就红」改成「缺了就 skip」，理由是代价落错了人：README 教所有人
跑 `xcodebuild … test`，而刚 clone 的贡献者手上不可能有这份 fixture，第一件事就得到一片红
然后去查一个不存在的故障。`XCTSkip` 不是静默的 —— 报告里是独立状态，并原样打出带修复
命令的 hint。

> 因为这三个原因都跟环境有关，**本机（开发者机器，fixture 在）跑出来是 3 skip 而不是 11**。
> 报数字的时候说清是哪台机器。

---

## 10. 正在演进中：`Sources/Mac/` 的前后端分离

⚠️ **这一节描述的是一个中间态，不是终态。目标形态见
`docs/internal/2026-08-19-backend-split-design.md`。**

**要解决的问题**（人类原话）：「每次更新我都得等 session 完成工作，有没有办法重启
PendingCrew 之后能恢复 session 而不用等它？就像休眠而不是关机。」

**选定的做法**：session 不再由 app 进程养，改由一个常驻后台进程（同一个二进制的第三副
身份 `--daemon`）养；app 退化成「连上去看的那个窗口」。顺带从结构上解掉 `docs/tech-debt.md`
第一条（PTY 每批输出都过主线程、代价随 session 数线性涨）。

六个阶段，**当前 main 上 P0 与 P1 已落地，P2 及以后未开工**（核对方式：全仓 grep 不到
`RemoteSessionBackend` / `InProcessTransport` / `SessionTransport`）：

| 阶段 | 做什么 | 现状 |
|---|---|---|
| **P0** 所有权归拢 | 长期职责从视图摘下来交给 app 级 `SessionHost` | ✅ `Sources/Mac/Services/SessionHost.swift`、`Sources/Mac/LocalRunner/ProcessRole.swift` 已在 |
| **P1** 终端劈半 | `AgentTerminalSession` → 无画面 `AgentSessionCore` + 只负责画的 `TerminalMirrorView` | ✅ 三个文件都在，`AgentTerminalSession` 已退化成 162 行的薄门面 |
| **P2** 协议 + 进程内传输 | 定义全部消息、`RemoteSessionBackend` 走传输层 | ⬜ 未开工 |
| **P3** 快照 + 背压 | 终端缓冲区快照序列化（全项目风险最高的一块） | ⬜ 未开工 |
| **P4** 真进程分家 | `--daemon` 身份、Unix socket、编排搬进 daemon | ⬜ 未开工 |
| **P5** 常驻与善后 | `SMAppService.agent` 登录项、菜单栏项、孤儿回收 | ⬜ 未开工 |

**P0/P1 已经改变了什么，读代码时要知道**：

- 长期服务（`CrewSessionRunner` / `CrewRelayAgent` / `LocalAgentUsageMonitor` /
  `CrewLocalMentionWaker` / `QuotaCenter` / `ModelCatalogCenter`）由
  `PendingCrewApp` 上的 `@StateObject sessionHost` 单一持有。**视图只观察，不创建。**
  `SessionHost.start()` 第一行是一条 `precondition(ProcessRole.current == .orchestrator)`
  ——防双头，宁可当场崩也不悄悄跑成两个所有者。
- `AgentSessionCore.swift` **不许 import AppKit / SwiftUI**，有测试盯着这件事。
  新加终端相关逻辑时想清楚它属于「内核」还是「画面」。
- `ProcessRole.resolve` 兜底选 `.orchestrator` 而不是 `.viewer`：总闸拼错时
  「没人管账」比「两个人管账」更难发现。

**一处 P0 顺带查出的实况**（已登记 🔴）：`CrewMailboxWaker` 与 `SessionPermissionRelay`
这两个服务原本由视图接线，两个调用点都在死路上，**一次都没被调用过**。登录态下 edge
信箱的定向投递与远端审批镜像因此从未生效——静默不工作，没有任何报错。P0 只删掉了视图侧
那段永不执行的接线，实现原样留在 runner 上。归属 P4 或云端那条轴。

---

## 11. 「我想改 X，去哪个文件」

| 我想改… | 去 |
|---|---|
| 加一个 agent 能调的 crew 工具 | `Sources/Mcp/McpServer.swift`（`tools/list` + `tools/call` 两处都要加）；要 app 干活就再加一条控制通道命令（`Sources/Stores/LocalCrewControlStore.swift` + `Sources/Stores/CrewStore.swift` 的排空侧） |
| agent 启动时看到的世界观 / 机长 persona | `Resources/Prompts/*.md`（模板）+ `Sources/Stores/LocalSessionWorldModel.swift`（填哪些变量） |
| claude 的命令行参数 | `Sources/Mac/LocalRunner/SessionConfig.swift` 的 `argv()`（有 `SessionConfigTests` 盯着） |
| claude 的 hook 接线 | `Sources/Mac/LocalRunner/LocalSessionLaunch.swift` 的 `prepareLocalCommsConfig` |
| codex 的协议交互 | `Sources/Mac/LocalRunner/CodexAppServer/CodexProtocol.swift`（参数）/ `CodexAppServerBackend.swift`（行为） |
| 终端的行为（PTY / 状态扫描 / 回滚） | `Sources/Mac/LocalRunner/AgentSessionCore.swift`（**不许 import AppKit**） |
| 终端的画面 | `Sources/Mac/LocalRunner/TerminalMirrorView.swift` |
| session 的健康判定（未登录 / 撞额度） | `Sources/Mac/LocalRunner/SessionHealth.swift` |
| 「它卡在菜单上等人选」的识别 | `Sources/Mac/LocalRunner/SessionPendingDecision.swift` |
| 白板消息的字段 | `Sources/Stores/LocalWhiteboardStore.swift` 的 `LocalWhiteboardMessage`（**加字段必须给默认值** —— 旧 JSON 缺键要能解） |
| 谁被 @ 了 / 谁该被唤醒 | 判定 `Sources/Mac/LocalRunner/CrewLocalMentionWakeLogic.swift` + `Sources/Stores/CrewWhiteboardVisibility.swift`；编排 `Sources/Mac/Services/CrewLocalMentionWaker.swift` |
| 群聊气泡长什么样 | `Sources/Chat/Vendored/BubbleView.swift`（**vendored，先读文件头**）；映射在 `Sources/Chat/Adapter/CrewChatAdapter.swift` |
| Markdown / 公式渲染 | `Sources/Chat/Vendored/MarkdownText.swift` / `MathRendering.swift`（同样 vendored） |
| 三栏主界面 | `Sources/Mac/Views/MacRootView.swift` → `CrewSidebarView` / `CrewChatView` / `CrewSessionWindowView` |
| iPad / iPhone 形态 | `Sources/Views/IPadShell.swift`（detail 直接复用 `CrewChatView`） |
| 侧栏状态点的颜色语义 | crew 级 `Sources/Support/CrewStatusAggregation.swift`；session 级 `Sources/Support/SessionStatusDot.swift`（**两套，别混**） |
| 额度环 | 判定 `Sources/Support/QuotaRingLayout.swift`；取数 `Sources/Mac/Services/QuotaCenter.swift` |
| 起 session 时的 git worktree | `Sources/Mac/LocalRunner/GitWorktreeService.swift` |
| 改 crew 工作目录（含 agent 上下文迁移） | `Sources/Mac/LocalRunner/WorkdirMigrationPlan.swift`（规划）/ `WorkdirMigrationExecutor.swift`（执行） |
| 自动更新行为 | `Shared/AppUpdate/AppUpdater.swift` + `UpdateCheckGate.swift` |
| 发版流程 | `scripts/release/build-macos-update.sh` + `docs/release-macos.md` |
| 云端后端坐标 | `Sources/Services/CrewHostedConfig.swift`（四个常量；`isConfigured` 是唯一真值，**别动 `Placeholder`**） |

---

## 12. 已知的债

结构性问题登记在 `docs/tech-debt.md`（按 🔴 根基级 / 🟡 拆东墙补西墙 / 🟢 不规范 分档）。
本次梳理新登记了四条，都在那个文件里：

- ~~🟡 **没有 CI**~~ —— **已还（2026-08-21，`.github/workflows/ci.yml`）**
- 🟢 **`Sources/Mac/` 名不副实 + 单模块没有编译期的层**（第 4、5 节的两处发现）
- 🟢 **`whiteboards/` 目录只增不减** —— per-session 的 `.cursor` / `.turn` / `.lock` 从不回收；
  本机实测两天从 1051 涨到 1346 个文件，而主线程仍在列这个目录
- 🟢 **驾驶舱有一半的数据契约只存在于另一个仓库** —— `docs/handbook/` 与 `docs/state/`
  的格式这里没写过，空态却直接指向一个未开源的仓库

另有一条与阅读代码直接相关、但**不是债**的背景：**这个仓库的 git 历史只有 50 个提交、
最早一条是 2026-08-15** —— 2026-08 仓库重建过一次，之前三千多个提交不在这里。所以
`git log` / `git blame` 查不到某段代码的来历是正常的，**注释里的日期和现象才是这个仓库的
真历史**。这也是发版脚本的 build 号从「git 提交数」改成「时钟派生」的原因
（`build-macos-update.sh:31-50`）。
