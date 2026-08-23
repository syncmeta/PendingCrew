cask "pendingcrew" do
  version "0.1.13"
  sha256 "5c7c1659277cb4c5f0d57218f0dcb87764acfe157f6cc7c564405fccf79a04d5"

  url "https://github.com/syncmeta/PendingCrew/releases/download/v#{version}/PendingCrew-#{version}.dmg"
  name "PendingCrew"
  desc "Run multiple coding agents (Claude Code / Codex) as one crew"
  homepage "https://github.com/syncmeta/PendingCrew"

  livecheck do
    url :url
    strategy :github_latest
  end

  # 这个 app 自带 Sparkle 自更新（Shared/AppUpdate/AppUpdater.swift 里的
  # SPUStandardUpdaterController，设置里有「检查更新」按钮，会真的下载并安装）。
  # 不声明的话 brew 会以为版本落后而去覆盖安装，和 Sparkle 打架 —— 用户会看到
  # 已经装好的版本被莫名回退或者重复下载。
  auto_updates true
  depends_on macos: :sonoma

  app "PendingCrew.app"

  # 普通卸载只退进程 + 删 app，**不碰任何本地数据**：白板、群聊记录、Todo、
  # 审批账本都在下面 zap 段列的目录里，那是用户的东西。
  uninstall quit: "com.pendingname.pendingcrew"

  # zap 只在 `brew uninstall --zap` 时才跑，默认永远不执行。
  # 这里列的全部是 app 自己创建的目录（含它自己的安装器/回滚位/备份位），
  # 不含用户自己另存到别处的文件。
  zap trash: [
    "~/Library/Application Support/PendingCrew",
    "~/Library/Application Support/PendingCrew-app-rollback",
    "~/Library/Application Support/PendingCrew-databackup-*",
    "~/Library/Application Support/PendingCrew-installer",
    "~/Library/Application Support/PendingCrew-rescue-*",
    "~/Library/Application Support/PendingCrew-whiteboard-backups",
    "~/Library/Caches/com.pendingname.pendingcrew",
    "~/Library/HTTPStorages/com.pendingname.pendingcrew",
    "~/Library/Preferences/com.pendingname.pendingcrew.plist",
  ]
end
