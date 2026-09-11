# #91：本地图缓存测试只约束本仓逻辑

## 根因与修复

原来的 `testStoreThenPeekHits`、`testDifferentMaxPixelIsDifferentEntry` 用默认 NSCache，存入后断言一定还在；这与源码已声明的可驱逐行为冲突。历史内存读数可在 `2026-09-11-typing-lag-profile.md:20` 复核，但那次测试失败的原始日志本轮未找到，不能独立确认当时每次失败都由驱逐造成。本轮用立即丢弃写入的存储确定性复现这两条断言失败。

新增 `CrewLocalImageStorage`，默认适配器只转发 NSCache 的 object / setObject(cost:) / removeAllObjects，仍设置原来的 64 MiB 默认成本上限。key、成本公式、解码方法均未改变。测试注入受锁保护的普通字典，生命周期由测试掌握；另测立即驱逐允许 miss。覆盖测试证明旧条目还在但新 key 不命中；分桶测试证明同时存原图和缩略图不会互相覆盖。接线测试直接检查注入存储收到正确 key、成本、替换、清空，避免只看 peek 而漏掉注入点失效。

## 它被谁调了

- `Sources/Mac/Support/CrewLocalImageCache.swift:7`：存储协议；生产适配器在同文件 `:14` 实现，内部 `:15` 是 NSCache，`:17` 设置原成本上限。
- 同文件 `:77` 的新 `storage` 参数默认 nil，`:79` 选择生产适配器；`:41` 的 `shared = CrewLocalImageCache()` 走这一默认路径。显式非 nil 只由测试使用，注入时存储自行管理容量。
- `Sources/Mac/Support/CrewImageLoader.swift:50` 计算 key，`:57` 调用 shared.peek，命中在 `:60` 返回，绕开 `:65` 后台解码；`:70` 将解码结果写回 shared.store。
- 缓存 `:84` / `:88` / `:93` 分别转发查询、带成本写入、清空。生产调用链属于源码接线核验，本轮没有启动 GUI。
- 未新增 Swift 文件，故不需生成工程；现有源码和测试条目已在工程里。

## 验证记录

日志工作目录：`/tmp/crew-cache-91-evidence/`。

1. `01-eviction-red.log`：沙箱 DNS 解析失败，没有执行测试，不算功能红。
2. `02-eviction-red.log` + `02-eviction-red.patch`：保留原测试断言，仅加入注入接口并把测试存储设为立即驱逐。Executed 6 tests, 2 failures；具名为上述两条，xcodebuild exit 65。
3. `03-fixed-green.log`：修复后 Executed 8 tests, 0 failures，exit 0。随后提交 `5ed062d`，再开始变异。
4. `04-ignore-injection.patch` / `04-mutation-red.log`：只将初始化中的 `storage ??` 移除，强制使用生产 NSCache，保留参数使其正常编译。Executed 8 tests, 4 failures，exit 65；2 个测试失败：允许驱逐测试的 nil 断言，以及接线测试的对象/800/1 三个断言。后者与 NSCache 是否驱逐无关，稳定检出未写入注入存储。
5. 使用 `cp /tmp/crew-cache-91-evidence/cache-before-mutation.swift Sources/Mac/Support/CrewLocalImageCache.swift` 还原，`git diff --exit-code` 为空；`05-restored-green.log`：Executed 8 tests, 0 failures，exit 0。
6. `git worktree add --detach /tmp/crew-cache-91-accept 5ed062d` → `cp -R` 共享 `Tests/PendingCrewTests/Fixtures` → status 为空；运行前 `df -h /` 为 22 GiB。`sh scripts/test-mac.sh /tmp/crew-cache-91-accept` 使用其 `.test-archive/dd` 独立 DerivedData。`06-full.log`：Executed **2607 tests, 3 skipped, 0 failures**，151.797 秒，exit 0。缓存 8 条全部通过，另按逐条日志核得 2604 passed + 3 skipped = 2607。跳过为默认不执行的 `AgentTuiFixtureRecorder.testRecord` 及未提供现场目录的 `CrewLastMessageCacheTests.test_基准_现场白板目录`、`SessionAwaitingReplyInputsCacheTests.test_基准_现场目录`。
7. 全量前后 detached HEAD 均为 `5ed062d8444a96548ce4d9704f2d4b44bfacc1e0`，status 均为空。完整日志和 `06-full.xcresult` 已复制到上述证据目录，然后删除自有 `.test-archive` 和定向 DerivedData；清理后磁盘 23 GiB。本记录后续提交仅改文档/证据，不改已验收源码。未 merge、未 push。

## 边界

- 字典测试验证缓存的 key/成本/对象返回合同，不测 NSCache 内存压力、驱逐时机或成本上限触发阈值。生产仍可 miss 并重解码；“同 key 不重解”以条目仍保留为前提，不承诺并发请求去重。
- UI 命中分支跳过解码是源码核验，未做 GUI 或真实重绘时解码次数观测；不把缓存单测当成端到端性能证明。
- 协议要求线程安全，适配器依赖 NSCache，测试字典加锁；本轮没有压力并发/TSan 测试。
- 覆盖失效依赖 mtime 或 size 改变。同路径内容改变但二者均未变仍可能返回旧图，这是原有 key 合同边界。
- PNG 写入、ImageIO 解码、临时目录和 AppKit bitmap context 仍依赖系统；磁盘满、权限、编码器变化或进程资源耗尽仍可能导致测试环境失败。字典避免驱逐假红，不使测试免疫所有环境故障。
- 原成本公式依据 NSImage.size；极端非有限/溢出尺寸、DPI 与真实像素差异未扩展处理，不改变生产语义。

## 复跑命令

```sh
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' -derivedDataPath /tmp/crew-cache-91-dd -only-testing:PendingCrewTests/CrewLocalImageCacheTests test
```

确定性先红 patch 从基线 `272282d` 应用；忽略注入 patch 从 `5ed062d` 应用。两份 patch 及各轮日志摘要在 `samples/2026-09-12-local-image-cache/`。完整日志另以群附件留存；全量 xcresult 保留在 `/tmp/crew-cache-91-evidence/06-full.xcresult`（未塞进附件压缩包）。
