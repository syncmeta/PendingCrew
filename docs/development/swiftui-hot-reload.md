# SwiftUI 保存即热更新（InjectionNext）

当前：**真实 PendingCrew 开发窗口的保存热更新已实测通过**。在同一 App 进程中保存生产 `CrewSidebarView` 的文案修改，QA 看到了界面变化，且“消息”选择状态保持；随后原样保存恢复。当前开发窗口保留，日用 App 未动。

## 开发入口

在这份源码工作树中执行一条命令：

```sh
python3 scripts/dev/swiftui-hot-reload.py run
```

`run` 已完成本机暖构建、复用既有工具并打开真实窗口的端到端实测。它复用既有 PendingCrew target / 暖 DerivedData / SourcePackages；首次构建，复制已安装 App 的 Core 到独立开发包（保留 Core 原字节和签名，仅签开发外层），使用独立偏好域与 `/private/tmp` 数据根，准备内存开发身份，然后启动或复用已验证的 InjectionNext，打开真实 `PendingCrewApp → RootView → MacThreePaneView`。不会安装到 `/Applications`，不会退出任何现有进程。若开发 App 已运行，仅在已有运行记录的 PID、产物与源码工作树都匹配时显示该来源；来源缺失或不同 checkout 时明确拒绝继续，不冒称当前目录可热改，也不重复启动或覆盖产物。

本机默认沿用已经准备的官方工具、暖缓存和私有运行根；其他位置可用 `--injection-app`、`--derived-data`、`--backend-input`、`--runtime-root` 指定。工具须是官方签名的 2.0.1。`run` 自动配置环境，无需手填多个变量。真实 Core 的 Codex/Claude 程序均明确设为 `/usr/bin/false`，远程凭据使用现有 memory seam，个人草稿使用现有 `owned(root:deviceIdentity:configuration:nil)` 接口，不创建 Keychain 身份或连接 CloudKit。这是 UI 开发组合，不是新签发的完整产品候选。

编辑 `run` 打印的**当前工作树**下生产 Swift 文件并保存。另一个 checkout 的源文件不会自动变成这个进程的代码。先保持窗口、进程和导航位置，观察注入日志及同一窗口变化；结构变化时正常退出这个开发 App，再执行 `run` 做暖增量构建。它不会替你终止日用 App。

只准备工程或构建、不启动：

```sh
python3 scripts/dev/swiftui-hot-reload.py prepare
python3 scripts/dev/swiftui-hot-reload.py build
```

生成工程在 gitignored `.hotreload/PendingCrew.xcodeproj`，只有现有 target 的 macOS Debug 配置，直接引用生产 Sources。正常 `project.yml` / pbxproj / Release / profile / 正式 entitlements 不改。开发配置启用 `-Onone`、`-Xlinker -interposable`、`EMIT_FRONTEND_COMMAND_LINES=YES`，仅这里关闭 compilation cache。Git HTTP/1.1 只作用于构建子进程，不改全局配置。构建前要求至少 3 GiB 可用空间，不会删除缓存；这是针对既有 target/依赖/SDK 全暖的保守门槛，本轮现有 target 中间物与 App 合计约 426 MiB，并非历史构建净耗盘测量。

## 使用范围

已按官方方式接线的真实生产 View：`MacThreePaneView`、`CrewSidebarView`、`CrewCenterView`、`CrewdGlassWorkspaceTabs`。各自使用 `@ObserveInjection` 与 body 末尾 `.enableInjection()`，观察注入通知并作 `AnyView` 类型擦除；条件为 `DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)`。普通 Debug/iOS/Release 不导入 Inject。

函数/body 内文案、颜色、间距及 ABI 不变的实现可尝试保存注入。添加/删除/重排存储属性、更改类型/函数签名、非 final class 方法布局、资源/依赖/编译条件变化仍须正常退出开发进程并暖增量 Build & Run。父 View 有 hook 不保证所有后代自行刷新；修改其他 View 时只给实际要迭代的生产 View 加同样官方 hook，不另写 demo。Previews 只作尺寸/组件检查辅助。

工具使用 localhost `127.0.0.1:8887`，devices/MCP/自动启动 Xcode 关闭；不启用 multicast、真实设备或 compiler proxy，不修改 Xcode 工具链。采用官方 Watch 模式和既有构建日志；watch 指向真实 Sources，避免官方自动改写普通项目的链接设置。开发模块名必须与执行文件名 `PendingCrewHotReload` 一致，供官方日志 fallback 按 `-module-name` 匹配。

## 官方版本

- [InjectionNext 2.0.1](https://github.com/johnno1962/InjectionNext/releases/tag/2.0.1)：源码 `2bb9fd51d7afd1f58e3000f09071bd3bdad9c527`（annotated tag object `2208febb21c18d8e15b00b178c885584a9941ae7`）；官方 release ZIP SHA256 `7390db00a82bebf6fa2b828f28e40d29b12d551e4c749a15779ce79eae1d9737`。本次下载匹配摘要，strict/deep 签名校验通过，Developer ID John Holdsworth / `9V5A8WE85E`，公证票据已 stapled。
- [Inject 1.6.0](https://github.com/krzysztofzablocki/Inject/tree/f53380e986a8b056e37d7ce9ca68de610b8415ef)：固定源码 `f53380e986a8b056e37d7ce9ca68de610b8415ef`，仅可选开发工程添加。按官方 bundle 加载方式从指定 InjectionNext resources 加载 `macOSInjection.bundle`，不再重复添加 InjectionNext SPM 客户端。
- 本机工具链实际读取为 Xcode 27.0 / 27A266a；官方 release 标明支持 Xcode/iOS 27，匹配标题不代替项目实测。函数替换/重绘限制见官方 [README](https://github.com/johnno1962/InjectionNext/blob/2bb9fd51d7afd1f58e3000f09071bd3bdad9c527/README.md) / [INTRO](https://github.com/johnno1962/InjectionNext/blob/2bb9fd51d7afd1f58e3000f09071bd3bdad9c527/INTRO.md)。

## 2026-10-23 实测记录

1. gate/生成工程测试通过，普通构建对象无开发 gate 符号；四个生产 View 的开/关条件 Swift parse 通过。固定 Inject 源码在当前 Xcode 编译为模块成功。
2. 首次 SPM 解析遇 HTTP/2 framing error；命令级 HTTP/1.1 解决。完整 Debug target 构建成功，后续 runtime 接线和目录修正的暖增量构建也成功。
3. 首次入口因 Foundation 对已存在 `/private/tmp` 目录返回 `/tmp` 而误拒；改为 native `realpath` 后，实际现有两根正例及日用路径/穿越/symlink 负例通过，真实原窗口成功打开、本机空库连接正常。
4. 首轮保存“通讯录→热更新”在同一 App/工具 TCP 已连接时失败，界面仍是原文。官方固定 [LogParser](https://github.com/johnno1962/InjectionLite/blob/20dd8459d058a6012ea156eecdeef586260ad90d/Sources/InjectionLite/LogParser.swift) 的 `-primary-file` 查找无匹配后，fallback 用执行文件名匹配 `-module-name`；实际日志模块是被多余 override 固定的 `PendingCrew`，执行文件却是 `PendingCrewHotReload`。已移除 override，临时文案原字节恢复。该首轮失败证据保留；之后已重新构建修正并完成下述成功验证。
5. 正常退出开发窗口曾被自动审批拒绝，当时立即停止且保留现场；用户随后明确授权本轮私有开发窗口退出、重开和验证。QA 正常退出旧开发 App/Core 后，`run` 暖构建成功，新 App PID `76659`，原工具 PID `53720` 保持，Core SHA256 仍为 `09b4b0c6eb04f41c825bc400ee21c3e8cc386cbcdd7c1d9e4bfdf41f7f70276f`。
6. UTC 12:06:14.377910 保存生产 `CrewSidebarView.swift` 的“通讯录→热更新”，客户端实际编译 1409 ms，热替换 3 个符号。QA 于 12:06:46.549–.721 在同一窗口确认“热更新”可见、“消息”仍选中。12:07:13.453865 原字节保存恢复，客户端编译 1118 ms 并再次替换 3 个符号；QA 于 12:07:34.279–.459 确认“通讯录”恢复、“消息”仍选中。全程 App PID `76659` / 私有 Core PID `76666` 保持，私有库的 session/receipt 均为 0，工具仅监听 `127.0.0.1:8887`。这里的毫秒是工具报告的编译耗时；GUI 观察有取证间隔，未精确测量保存到屏幕的完整延迟。结构变更仍须重建；本轮未用结构修改破坏运行窗口来测试。
