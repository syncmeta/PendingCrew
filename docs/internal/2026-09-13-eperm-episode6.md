# 数据目录 EPERM 第 6 次发作（2026-09-13 上午）

接 `2026-09-12-eperm-marker-travels.md`。那份里的排除项和绕路**不重查**。

## 实测到的

- **起点落在 11:13:27 到 11:24:54 之间**。11:13:27 是本 crew 白板最后一次
  写成功的时刻（stat 的 mtime，发作期间 stat 照常）；11:24:54 是我第一次
  `head -c 1` 读失败的时刻。能缩窄到这么多，再准就没有了。
- 11:24:54：`whiteboards/*.json` 共 165 个，读不动的 153 个。读得动的 12 个
  全是 10:25 之后新写出来的 plan / human-todos / captain-awareness / todos
  文件，印证「新写出来的不受影响，老文件受影响」那条。
- 顶层 `local-crews.json`、`daemon.registry.json` 读不动，而这两个文件 10:52
  之后被写过（mtime 在变）。所以**写路径照常，读路径被拒**，形状跟前五次一样。
- `post_to_crew` 回「白板读不出来，已存进待发件箱」，errno=1，读的进程是
  helper pid 21397。它跑在 0.1.37 新二进制上（inode 187757457，当天核过）。
  **所以换到新二进制救不了读已有文件**，这一点跟 9.1 那段对得上。
- 11:26:26 抓了一份发作中的进程表：`~/Library/Logs/PendingCrew-eperm/ps-episode6-seen-20260913-112626.txt`（964 行）。

## 这次挂上了恢复当口的守候

`scripts/watch-eperm-recovery.sh`（e7007ea），11:26:38 起守，pid 51692。
- **已实测**：ppid 1，会话号跟 claude 不同，是自己另开的会话。
- **推出来的、没验过**：本 session 被停时它还活着。
- 探针是 `local-crews.json`，每 10 秒探一次，最长守 24 小时。
- 恢复那一刻会：记下时刻，抓进程表跟起守时 diff，数一次读得动/读不动的文件，
  用 clonefile 把数据根克隆一份。
- 日志：`~/Library/Logs/PendingCrew-eperm/watch-20260913-112638.log`

## 还缺的

- 人类 Todo #21：发作当口 `sudo scripts/capture-eperm-fsusage.sh`。
  这一窗是第 6 个机会。
