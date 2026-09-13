# #91 重派现场复核

本次接单基线为 `5f02cb2`，独立工作树起始干净。派单称尚未实现，但现场确认 `5ed062d` 已是 HEAD 祖先，缓存源码与测试相对该提交无差异；已有存储注入、不驱逐字典、立即驱逐 fake 和 8 条测试。不重复改写已完成的实现。

## 已复核的历史证据

直接读取 `/tmp/crew-cache-91-evidence/` 的完整日志及 patch，不只依赖既有报告：旧 6 条测试在立即驱逐存储下有 2 个断言失败（命中、maxPixel）；修复后 8/0；去掉 `storage ??` 后 8 条测试有 4 个断言失败，包含注入字典未收到对象及成本 800/1；cp 恢复后 8/0。历史 detached `5ed062d` 全量为 2607 executed、3 skipped、0 failures。这是历史执行，本次执行另记。

原报告所述 `samples/2026-09-12-local-image-cache/` 不在当前树或 `8a1b256` 的 git tree 中；目前可核验 patch 位于上述临时证据目录。不能把该 samples 路径描述为已经提交的证据。

## 生产调用及边界

`Sources/Mac/Support/CrewLocalImageCache.swift:7` 定义协议；`:14` 的生产适配器包装 NSCache，`:17` 设置成本上限；`:77` 新 storage 参数默认 nil，`:79` 使用生产适配器，`:41` 的 shared 是生产构造入口。非 nil 注入只在测试使用，key、成本公式及解码均未变。

`Sources/Mac/Support/CrewImageLoader.swift:50` 算 key、`:57` 查 shared.peek、命中在 `:60` 返回，因此跳过 `:65` 的后台解码；未命中后在 `:70` shared.store。这是源码接线核查，未做 GUI 解码计数验证。

字典隔离了 NSCache 的驱逐不确定性，保留条目时同 key 返回同对象，原图与缩略图可同时存在。它不保证生产必命中，不验证 NSCache 压力阈值、并发请求去重或 TSan。mtime 与 size 同时未变的覆盖可能仍命中旧内容。PNG 编解码、临时目录权限、磁盘满或进程资源耗尽仍可能导致环境假红。未增加 Swift 文件，不需生成工程。

## 本次验证

干净 detached 验收树 `/tmp/crew-cache-91-recheck-5f02cb2`，由 `git worktree add --detach ... 5f02cb2` 创建，并 `cp -R` 共享 Fixtures；复制后 status 为空。开跑前磁盘可用 15 GiB。全量通过 `scripts/test-mac.sh`，DerivedData 在该树自己的 `.test-archive/dd`。首次尝试被沙箱 DNS 阻断，没有执行测试，不算红；获准后重试。

本轮日志目录 `/tmp/crew-cache-91-recheck-evidence/`。结果待运行完成后补入。
