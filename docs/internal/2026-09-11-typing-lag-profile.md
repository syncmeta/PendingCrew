# 打字不畅顺现场读数（人类 Todo #140）

- 基准提交：`ba0cb07`
- 采样时刻：2026-09-11 17:17 / 17:23，**症状正在发生时**采的
- 采样手段：`sample <pid> 3` / `sample <pid> 5`（纯命令行，没碰界面）
- 原始样本：`/tmp/pc-sample.txt`、`/tmp/pc-sample2.txt`（临时目录，会被回收）

## 结论：两件事同时成立，主因是第一件

### 一、整机内存已经吃满，在换页（主因）

| 读数 | 值 |
|---|---|
| 物理内存 | 32 GB |
| free | 0.9 GB |
| 压缩器占用 | **13.3 GB** |
| 交换已用 / 总量 | **5.35 GB / 6.0 GB** |
| Pageins（开机至今） | 4938 万 |
| load average (1/5/15) | 4.34 / 5.66 / 5.95（10 核） |

压缩器压着 13.3 GB、交换只剩 798 MB 时，**每一次敲键都可能撞上一页被压缩的内存**，
解压那一下就是手感上的顿挫。这跟哪个 app 写得好不好无关，是全机现象。

谁占着：

| 组 | 进程数 | 常驻内存 |
|---|---|---|
| claude / codex | 75 | 6.1 GB |
| PendingCrew 家族 | 29 | 1.1 GB |

**这是我们自己造成的** —— 同时跑着二十来个 crew 的 agent session。

> 注意：常驻内存不含被压缩掉的部分，所以这两行是**下界**，不是它们真实的占用。

### 二、聊天窗主线程本身也忙（第二位，但可修）

两趟采样，主线程忙的比例分别是 **77%** 和 **45%**（差别很大，说明是阵发的，
不是恒定负载）。按模块拆独占耗时：

| 模块 | 第一趟 | 说明 |
|---|---|---|
| libswiftCore | 38.4% | 引用计数 / 哈希 / 字符串切片 |
| libicucore | **7.1%** | 全部来自反复新建 `ISO8601DateFormatter` |
| libswiftObservation | 6.4% | `AnyKeyPath` 哈希 —— `@Observable` 依赖集很大 |
| SwiftUICore | 3.7% | 布局 |
| PendingCrew 自己 | 1.1% | 自己的代码很薄，贵在它调起来的框架活 |

三处具名的浪费（按「最没道理」排序）：

1. **`CrewMemberOrdering.parseDate`**（`Sources/Chat/Adapter/CrewMemberOrdering.swift:30`）
   每个成员每次排序都**新建一到两个 `ISO8601DateFormatter`**。这个构造要开 ICU 的
   日期格式器、locale、数字格式器，很贵。调用方 `CrewSessionWindowView.memberRowItems`
   是计算属性，每次 body 求值都重排一遍。ICU 那 7.1% 全在这儿。
   同样的写法还在 `CrewMessageSearch.parseISO`（:110）。

2. **`CrewMentionFilter.bodyMentionsHuman`**（`CrewMentionFilter.swift:177`）
   每条消息都重建一次 `Set(roster.humanNames.map { $0.lowercased() })`，
   然后每遇到一个 `@` 就把花名册全名单扫一遍做 `range(of:options:)`。
   本群现状：**2618 条消息、正文 965 KB、正文里 814 个 `@`**。

3. **`CrewChatView.timelineEntries`**（`CrewChatView.swift:803`）
   计算属性，每次 body 求值都对**全部 2618 条**跑一遍上面那个过滤，
   而后面 `timelineRows` 只取最近 `renderLimit` 条。源码注释自己写了
   「这个属性每次访问都重算，所以判定必须廉价」—— 在 2618 条这个量级上它不够廉价。

## 排除掉的

- **不是构建在抢 CPU**。实际只有 1 个 `xcodebuild` + 2 个 `SwiftBuild`。
  （我第一次用 `pgrep -fl` 数出 286 个，那是模式匹配到了 shell 包装进程，是我数错了。）
- **不是白板解码慢**。整块 2.6 MB 解一次 JSON 只占主线程 13/1457 帧（0.9%）。

## 没盖住的

- **WindowServer 44%～52% 没有归因。** 它是画界面的进程，我怀疑跟内存换页和
  终端不停刷字都有关，但**没有证据**，不当结论。要分开量必须关掉窗口再量一次，
  那要人在场配合。
- 第二趟采样里 ICU 掉出了前八 —— 说明日期格式器那笔是**阵发**的（成员列表重排时才发），
  不是每一帧都在烧。所以「修掉它能回收 7%」这句话只在它发作的那段时间成立。
- 两趟采样都只在同一个五分钟窗口内采的，没有对照组（机器不忙时没量过）。
