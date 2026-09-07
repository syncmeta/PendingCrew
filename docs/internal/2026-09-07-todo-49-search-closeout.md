# Todo #49「聊天记录搜索」收尾核对

- **日期**：2026-09-07
- **核对人**：「聊天记录搜索」crew 机长
- **背景**：本 crew 静默 191 小时（定向 @ 在后台进程重启时被作废）。期间 #49 由别的
  session 独立完成并落 main。这份文件把「谁落了什么 / 还缺什么 / 哪条旧结论是假的」钉住，
  免得下一个人再从群聊里翻。

## 一、已落 main 的实现（不是本 crew 做的）

### PendingCrew

`20fc52a0f91ce9ee9773f1146baab2399ee2d998` — `feat(chat): add shared crew message search`
（2026-08-30 09:22，已在 main）

| 面 | 落点 |
| --- | --- |
| 共享逻辑 | `Sources/Chat/Adapter/CrewMessageSearch.swift`（纯 Foundation，跨平台）+ `CrewMessageSearchAdapters.swift` |
| 群内搜 | `Sources/Mac/Views/CrewChatView.swift` |
| 全局搜 | `Sources/Mac/Views/CrewGlobalSearchSheet.swift` + `CrewCenterView` / `CrewSidebarView` 入口 |
| session 工具 | `Sources/Mcp/McpServer.swift` 新增 `search_whiteboard`；`read_whiteboard` 同时补上分页（默认 50 / 上限 200 / `before` 游标） |
| 单测 | `Tests/PendingCrewTests/CrewMessageSearchTests.swift`、`Tests/PendingCrewTests/McpServerTests.swift` |
| 契约 | `docs/internal/chat-search-contract.md` |

语义按当初定的走：空白拆词 → 每词都要命中（AND）→ 大小写/音标/全半角折叠后做
**子串包含**，中文不另行分词。

### PendingBot

`8c2070dbbff7e1c53ab8031edad5de16d2f0e1b9`（merge `2cfdb4b7`）—
`feat: add full-history message search`（2026-08-30 10:14，已在 PendingBot 仓 main）

edge 路由 `apps/edge/src/routes/message-search.ts` + 迁移
`supabase/migrations/20260830013508_message_search.sql` + iOS
`MessageSearchView.swift` / `MessageTabView` / `ConversationView`。

服务端匹配用 `strpos(haystack, token) > 0`，haystack 先过
`message_search_normalize`（`unaccent` + `normalize NFC/NFKC` + `lower`）。
**即走的是子串包含，中文正确**，且搜的是服务端全量历史，不是本地缓存 —— 当初那条
「绝不允许"明明说过却搜不到"静默发生」的承诺，在**用户搜索**这条路上是兑现了的。

## 二、本 crew 的已落内容

**无。** 零 commit、零分支、零 worktree 含搜索实现；没有可 cherry-pick 的红测或实现。
worktree `.pendingcrew/worktrees/crew-Agent-Todo-49-PendingCrew-s-acef5d0e` 与 main 同点，
无自有改动。

## 三、仍然成立的缺口

### 1. 🟡 iOS / iPad 完全没有搜索入口（PendingCrew）

`20fc52a` 只动了 `Sources/Mac/Views/`。`CrewGlobalSearchSheet.swift` 第 1 行是
`#if os(macOS)`，`CrewCenterView` / `CrewSidebarView` 同样。
`Sources/Views/IPadShell.swift` 与 `Sources/Views/CrewListView.swift` 里 grep
`search` **零命中**。

原始要求写的是「四个界面形态都要考虑，别只做 Mac」。好消息是
`CrewMessageSearch.swift` 是纯 Foundation，**缺的只是 iOS 壳**，逻辑层可以直接复用。

### 2. 🟡 `search_whiteboard` 只有「本群」，没有「跨群」档

实现是 `store.list(crewId: crewId)`，只搜当前 crew。父机长当初的要求是
「至少支持本群和跨群」。

附带澄清，**别记成回归**：它没有过 `CrewWhiteboardVisibility`，但
`read_whiteboard` 本来也不过 —— MCP 拉取面一直是「本 crew 全量可见」，
收窄只发生在注入/唤醒面。真要加跨群档时，那时才必须接上可见性判定。

### 3. 🔴 bot 的 `search_chat_history` 中文仍然搜不到 —— 而且我们宣称的修复不存在

这是本 crew 要认的一笔账。群聊白板上记着「中文搜不到那个 bug 修好了，已合进 main
（`2cde0da9`）」。核实结果：

- **`2cde0da9` 在 PendingCrew 仓和 PendingBot 仓都不是合法 git 对象。**
- PendingBot 仓 `git log --all -S'chatSearchTerms'` **零命中** —— 那份「共享子串语义模块」
  从来没有进过这个仓库。
- PendingBot main 上 `apps/edge/src/lib/bot-reply/tools/search-history.ts` 今天仍是
  `q.textSearch('content_tsv', query, { type: 'websearch', config: 'simple' })`，
  第 12–13 行那句写错的注释 `works for cjk + ascii alike via word-bounded matching`
  **原样还在**。该文件在 main 上最后一次改动是 2026-05-12 的重构拆包（`c195b098`）。

分层说明（别把三层混成一层）：

- **读代码读到的（白纸黑字）**：上面三条，都是 2026-09-07 逐行读 main 得到的。
- **191 小时前实测过的**：`simple` 分词把中文整句当成一个 lexeme，所以搜「搜索」
  匹配不到「…讨论了搜索功能」。这次**没有重新在库上量**，但被量的那段代码一字未变。
- **推论**：因此 bot 工具的中文搜索现在大概率仍然静默返回空。要坐实，需要在真库上
  重跑一次当初那条断言。

**范围边界**：这个洞只影响 **bot 自己调的那个工具**，不影响 §1 里用户用的新搜索
（那条走 `strpos`）。

## 四、给下一个人的三条

1. iOS 壳复用 `CrewMessageSearch`，别再写第二套匹配逻辑（契约见
   `docs/internal/chat-search-contract.md`）。
2. 给 `search_whiteboard` 加跨群档时，可见性接 `CrewWhiteboardVisibility`，别另造。
3. 修 §3 时先在真库上跑一次中文断言**让它先红**，再改 —— 上一次就是没有这道红，
   才让一个不存在的修复在账本上挂了 191 小时。
