# SwiftUI 保存即热更新（InjectionNext）

当前状态：**源码接线已准备，真实运行窗口的热替换尚未验收**。这是现有 PendingCrew macOS target 和原 `PendingCrewApp → RootView → MacThreePaneView`，没有另写 demo。日用 App、发布工程和正式签名不变。首次仍须构建；后续受支持的函数体编辑才由 InjectionNext 单文件重编译、载入并触发 SwiftUI 刷新。

## 一条开发准备入口

从仓库根目录运行：

```sh
python3 scripts/dev/swiftui-hot-reload.py prepare
```

生成 gitignored `.hotreload/PendingCrew.xcodeproj`，源文件直接指向当前工作树。它只有现有 `PendingCrew` target 的 macOS Debug 配置；不创建新 target、DerivedData 或 SPM 根。`project.yml` 和正常 `PendingCrew.xcodeproj` 不改。默认复用此前协调的暖 DerivedData；其他机器须用 `--derived-data /absolute/existing/DerivedData` 指定已有目录。

首次前端构建入口（不安装、不启动）：

```sh
python3 scripts/dev/swiftui-hot-reload.py build
```

构建使用原暖 DerivedData/SourcePackages、保留原依赖的 resolved revision，新增加固定的 Inject。仅开发配置关闭 compilation cache、输出 frontend commands、使用 `-Onone` 和 `-Xlinker -interposable`；不会改全局 cache 或 Xcode 工具链。产品名 `PendingCrewHotReload.app`、bundle ID `com.pendingname.pendingcrew.hotreload`、ad-hoc、空 entitlements、不开 hardened runtime。开发构建不嵌入/复制日用 Core，不能当完整候选或发布包。

## 官方来源和固定版本

- [InjectionNext 2.0.1](https://github.com/johnno1962/InjectionNext/releases/tag/2.0.1)，annotated tag object `2208febb21c18d8e15b00b178c885584a9941ae7`，实际源码 commit `2bb9fd51d7afd1f58e3000f09071bd3bdad9c527`。官方 release 标明 Xcode/iOS 27；本机读取为 Xcode 27.0 / 27A266a。版本标题匹配不等于此项目实测通过。
- [Inject 1.6.0](https://github.com/krzysztofzablocki/Inject/tree/f53380e986a8b056e37d7ce9ca68de610b8415ef)，commit `f53380e986a8b056e37d7ce9ca68de610b8415ef`。只在生成的开发工程增加 SPM dependency。使用官方 `@ObserveInjection` 和 `.enableInjection()`，由其做通知观察和 `AnyView` 类型擦除，没有自造注入引擎。
- 本接线采用官方允许的 bundle 加载方式：Inject 从 InjectionNext App resources 加载 `macOSInjection.bundle`，因此不用再添加 InjectionNext 的 SPM 客户端，避免重复客户端。官方 [README](https://github.com/johnno1962/InjectionNext/blob/2bb9fd51d7afd1f58e3000f09071bd3bdad9c527/README.md) / [INTRO](https://github.com/johnno1962/InjectionNext/blob/2bb9fd51d7afd1f58e3000f09071bd3bdad9c527/INTRO.md) 说明三部分分别是重编译、函数绑定、重绘。

准备入口不会下载/安装/运行 InjectionNext App，也不启用设备端、网络端口、compiler proxy 或自动改写全部 SwiftUI 文件。

## 正常 Xcode 开发方式与尚未完成的运行层

通过可信的官方来源另外准备 InjectionNext 2.0.1 App 后，按官方 Launch Xcode 流程打开生成的项目。Xcode 的 Project Settings 中应把 DerivedData 设为上面相同的既有暖目录，不能让生成项目另造默认冷目录。也可以在已有暖构建日志基础上使用官方 Watch Project 模式；此模式需要 `EMIT_FRONTEND_COMMAND_LINES=YES`，本开发配置已设，但尚未在项目实测。

**当前生成的 scheme 不开启 App 运行。** `PendingCrewEntry.main` 在任何普通 GUI/Core 初始化前检查开发 runtime gate；没有经过私有运行审查的显式环境，进程只输出原因后返回。不得把在 Xcode 点 Run 退出称为热更新成功。

完整原 App 的 `.task` 会连接/启动 Core；个人草稿、配对、设备身份等可触及 Keychain；仅换数据目录不是副作用隔离。后续必须先确定可接受的私有 Core/身份/网络边界，准备确切的私有 Core（本构建不嵌入），再单独授权和验收真实运行。门禁要求独立开发 bundle ID、`PENDINGCREW_HOTRELOAD_PRIVATE_RUNTIME_REVIEWED=1`、两个显式 canonical `/private/tmp/` 数据根（`PENDINGCREW_DATA_DIR` / `PENDINGCREW_CREWD_DATA_DIR`）；这些条件**不是**网络或 Keychain sandbox，也不代表运行已经获准。不得用日用数据/配对凭据满足它。

运行层获准后，首次 Build & Run 应看到官方客户端连接日志；编辑已接线生产 View 的函数体，保存，观察 InjectionNext 重编译/注入日志和同一窗口更新。记录保存、注入完成、可见更新三个时点，并核对进程 PID/导航及未提交输入是否保留。当前未执行这一步，所以延迟、状态保持和失败恢复均无实测数字。

## 已接线的真实 View 与编辑边界

- `MacThreePaneView`：主窗口分栏和根修饰。
- `CrewSidebarView`：真实侧栏 body。
- `CrewCenterView`：真实工作区 body。
- `CrewdGlassWorkspaceTabs`：生产五域 tab 胶囊。

这四个 View 的 hook 都受 `DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)` 保护，普通 Debug/iOS/Release 不导入 Inject、不带观察器或类型擦除。编辑其他 View 时，按官方方式只给实际正在修改的生产 View 加同样 guard 下的 import、`@ObserveInjection`、body 末尾 `.enableInjection()`；父 View 有 hook 不保证所有后代自行刷新。无需重新创建另套展示界面。

适合热替换：函数/body 内文案、间距、颜色、布局逻辑及能保持 ABI 的实现修改。添加/删除/重排存储属性、更改类型或函数签名、非 final class 方法布局、依赖/资源/编译条件变化仍需停止开发进程后增量 Build & Run。首次构建和这些结构变化不可能靠保存完全替代。若注入失败，保留现场日志、修正源文件后再保存；持续失败则在私有开发 App 中回到正常增量构建，不重装日用 App，不 clean 全局缓存。SwiftUI Previews 仅可作为额外尺寸/组件检查，不代替原窗口验证。

## 2026-10-23 实际检查记录

- `python3 scripts/dev/test-swiftui-hot-reload.py` 通过：真实 Swift 门禁编译/负路径执行、普通/Release 编译对象无开发门禁符号、生成工程与原依赖 pin/签名配置不变量。
- 四个生产 View、入口和门禁文件在热更新开关开/关两种条件下均通过 Swift frontend parse；这不是完整 App typecheck。
- 固定官方 Inject 1.6.0 四个源码文件在 Xcode 27 使用既有暖 ModuleCache 成功编译为模块；未加载或执行该模块。
- `xcodebuild -showBuildSettings` 真实执行遇到 SPM 下载官方 Inject 的 `Error in the HTTP2 framing layer`，Xcode 自身重试后仍未完成；本次检查及其唯一下载子进程已终止，没有改代理/工具链/缓存策略。SPM 图解析、完整 App 编译仍未证实。未发起完整 App build。
- 未安装/运行 InjectionNext 或开发 App，未触碰日用 App/Core、配对/DB、模型/SDK。实际注入、重绘延迟、导航/草稿保持和结构修改后的增量构建回退均待私有运行层验证。
