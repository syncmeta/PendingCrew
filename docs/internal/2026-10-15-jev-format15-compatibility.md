# Jev 格式15兼容附录（wire02 JSON保持）

根2026-10-15裁定：正式Jev激活格式为15，按14→15升级；Native历史下一个正式格式预留16，当前不激活。wire02原快照、字段与JSON/JsonSchema不变；Known.result仅Rust Box布局补件独立保留。客户端不用改JSON解码，但只能在实际具备judgment能力的新Core使用配置，不能把旧Core14普通业务资格冒Jev资格。

兼容屏障必要性：准确旧Core14（31f0、Rust tree1014）work::followup_note把不认识的judgment-route唤醒视为Ordinary；旧render构造通用System正文、旧claim允许无捕获work_notes，可能ACK但没有新需求路由正文/源版本回验。extra中Settled事实回新Core后不得重投，故“无新DDL”不等于可安全raw降级。静态源证据/private/tmp/pcw-Jev-old14-compatibility-static-01保原blob，不冒实际旧消费测试。

schema15迁移在既有migrate_guarded保护备份后独立事务：PRAGMA user_version=15与meta judgment.format.v1=1成对提交，后者失败必须连同版本回滚。不增加表/索引，不触12/13/14冻结DDL、不带Native表/codec。未知格式标记不激活Runtime/能力或provider。旧14的既有version_guard必须在RW/pragma原文件触碰前拒15；不能依赖launcher约定或假registry制造拒启。官方受管降级需先正式恢复可用旧版本备份，不能直接旧程序开15。

必要实际门禁使用封存normal15/Core14（compiled31f0，binary48e83…22fc2）真实建立私有14库，而非PRAGMA伪造旧版。新无Native组合的诊断产物只作机制，不能称正式normal：验证备份障碍零原文件变化、格式标记写故障同事务回滚、保护备份存在、新15保业务ID/正文/receipt并重开、old14拒15时DB/id/WAL/SHM/journal字节与mode/inode不变、无公开socket。源fixtures/crewd-jev-format15-migration.py只启动自有false/false daemon，不启动provider/session/model/SDK/App/Keychain；全新0700目录保留现场，精确Popen有限TERM/KILL回执。源审和实际结果分别记录；此文不声称门禁已执行。

新Jev配置、pending/settled/Unknown/恢复见证及三业务需在15重新核必要影响；旧14机制证据只按不变源码复用，不重标。未来正式normal必须从新完整ref采真实所有输入/默认图：新增crewd-jev使预计12own包/13生产targets，最终按实际compiler-artifact/rawfresh证明而非固定旧393/11/12。Linux15受管release/cert另由client唯一构建作者与root调度；旧Linux14证据不能代新15。

作者实际证据追加：source02/action02以及输出/private/tmp/pcj15-migrate-owned-02已完成上述机制，actual baecb3/0，result SHA8ffd1f2bd462379847651385f730ecb32698d27f0588cd552a176706c294a78c。原source01 FAIL保留；source02通过真实old14停态排除synthetic writer的旁文件影响，不删除旁文件。此追加只记录Darwin私有机制结果，独立QA和未来正式新ref strong normal各自保留层级。
