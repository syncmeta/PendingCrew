//! crewd 的持久化层。crewd 是数据库的唯一写者；其他进程一律经 crewd 的 API。
pub mod archive;
pub mod backup;
pub mod channels;
pub mod crews;
pub mod db;
pub mod fsutil;
pub mod messages;
pub mod schema;
pub mod sessions;
pub mod testing;
pub mod todos;

pub use db::Db;

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

/// 存储本身出了故障（S26）：和「SQL 写错了 / 约束不成立」分开，人要看到的是原因，不是一个通用的内部错。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind {
    /// SQLITE_FULL：磁盘（或 max_page_count）满了。
    DiskFull,
    /// SQLITE_IOERR*：读写库文件时系统 I/O 出错。
    Io,
    /// SQLITE_CORRUPT / SQLITE_NOTADB：库文件损坏，或者根本不是库。
    Corrupt,
    /// SQLITE_READONLY：库（或它所在的目录）只读。
    ReadOnly,
    /// SQLITE_CANTOPEN / SQLITE_PERM：打不开库文件（权限、路径、EPERM）。
    CantOpen,
}

impl StorageKind {
    /// 给人看的一句话（不含路径、SQL）。
    pub fn describe(self) -> &'static str {
        match self {
            StorageKind::DiskFull => "磁盘已满",
            StorageKind::Io => "读写数据库文件时出现 I/O 错误",
            StorageKind::Corrupt => "数据库文件已损坏",
            StorageKind::ReadOnly => "数据库是只读的",
            StorageKind::CantOpen => "打不开数据库文件",
        }
    }

    /// 只看主错误码：扩展码（IOERR_WRITE、CORRUPT_INDEX…）都归到同一类。
    pub fn of(e: &rusqlite::Error) -> Option<StorageKind> {
        use rusqlite::ErrorCode as C;
        let rusqlite::Error::SqliteFailure(f, _) = e else {
            return None;
        };
        Some(match f.code {
            C::DiskFull => StorageKind::DiskFull,
            C::SystemIoFailure => StorageKind::Io,
            C::DatabaseCorrupt | C::NotADatabase => StorageKind::Corrupt,
            C::ReadOnly => StorageKind::ReadOnly,
            C::CannotOpen | C::PermissionDenied => StorageKind::CantOpen,
            _ => return None,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sql(rusqlite::Error),
    /// 存储故障（分类见 `StorageKind`）。`rusqlite::Error` 经 `From` 进来时自动归类。
    #[error("存储故障（{}）：{source}", kind.describe())]
    Storage {
        kind: StorageKind,
        #[source]
        source: rusqlite::Error,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
    /// 要找的行不存在（调用方要能和「其它失败」分开）。
    #[error("{0}")]
    NotFound(String),
    /// 库线程已经不在了（进程正在退出）。
    #[error("数据库线程已退出")]
    DbClosed,
    /// 交给库线程的闭包 panic 了；该闭包里的事务已回滚，线程照常服务。
    #[error("数据库任务 panic，已回滚")]
    DbJobPanicked,
    /// 库旁边的文件操作（快照、备份、标识文件）失败。
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// `quick_check` / `integrity_check` 没过（只报告：不删、不替换）。
    #[error("数据库没通过 {check}：{}", problems.iter().take(5).cloned().collect::<Vec<_>>().join("；"))]
    Integrity {
        check: &'static str,
        problems: Vec<String>,
    },
    /// 动手前预检：可用空间不够。
    #[error("可用空间不够：需要约 {needed} 字节，只剩 {available} 字节")]
    NoSpace { needed: u64, available: u64 },
    /// 快照（`VACUUM INTO`）没拍成；不留半截文件。
    #[error("快照 {} 失败：{source}", dest.display())]
    Snapshot {
        dest: std::path::PathBuf,
        source: Box<StoreError>,
    },
    /// 迁移前的保护步骤（完整性检查、快照）没过：没有迁移，库保持原版本、原样。
    #[error("拒绝迁移（库保持 v{from}，未改动）：{source}")]
    MigrationRefused { from: i64, source: Box<StoreError> },
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        match StorageKind::of(&e) {
            Some(kind) => StoreError::Storage { kind, source: e },
            None => StoreError::Sql(e),
        }
    }
}

impl StoreError {
    /// 是存储故障就给出类别（磁盘满、I/O、损坏、只读、打不开）；别的错误是 `None`。
    pub fn storage_kind(&self) -> Option<StorageKind> {
        match self {
            StoreError::Storage { kind, .. } => Some(*kind),
            StoreError::Integrity { .. } => Some(StorageKind::Corrupt),
            StoreError::NoSpace { .. } => Some(StorageKind::DiskFull),
            StoreError::Io(e) => Some(match e.raw_os_error() {
                Some(libc::ENOSPC) | Some(libc::EDQUOT) => StorageKind::DiskFull,
                Some(libc::EROFS) => StorageKind::ReadOnly,
                Some(libc::EACCES) | Some(libc::EPERM) => StorageKind::CantOpen,
                _ => StorageKind::Io,
            }),
            StoreError::Snapshot { source, .. } | StoreError::MigrationRefused { source, .. } => {
                source.storage_kind()
            }
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// 每次提交都走 `F_FULLFSYNC`（S23）。本机实测（N4 报告，Apple Fabric SSD，单行事务 ×400，两轮）：
/// 关 p50≈40µs / p99≈60µs（普通 fsync 在 macOS 上不保证落到盘片）；开 p50≈3.9ms / p99≈6ms。
/// 单写者每秒仍能做 ~250 次提交，远高于实际写入频率；而这个库是聊天记录的唯一一份，取强的那档。
pub const FULLFSYNC: bool = true;

pub struct Store {
    conn: Connection,
    /// 文件库的备份目录（`<库所在目录>/backups`）；内存库没有。
    backups: Option<std::path::PathBuf>,
}

impl Store {
    /// 打开（必要时新建）库文件。旧版本的非空库迁移前先在 `backups/` 拍快照（S20）。
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?, Some(backup::backups_dir_for(path)))
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(conn: Connection, backups: Option<std::path::PathBuf>) -> Result<Self> {
        pin_pragmas(&conn)?;
        schema::migrate_guarded(&conn, backups.as_deref())?;
        Ok(Self { conn, backups })
    }

    /// 备份目录（内存库是 `None`）。
    pub fn backups_dir(&self) -> Option<&Path> {
        self.backups.as_deref()
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn with_tx<T>(
        &mut self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let tx = self.conn.transaction()?;
        let v = f(&tx)?;
        tx.commit()?;
        Ok(v)
    }

    /// 没有任何机组、也没有任何消息。导入只往空库里写。
    pub fn is_empty(&self) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM crews) + (SELECT COUNT(*) FROM messages)",
            [],
            |r| r.get(0),
        )?;
        Ok(n == 0)
    }
}

/// 落盘相关的 pragma 一律显式设（S23），不靠编译期默认值。文件库拿不到 WAL 就拒绝打开
/// （比如落在不支持共享内存的文件系统上），别悄悄退回别的日志模式。
fn pin_pragmas(conn: &Connection) -> Result<()> {
    // journal_mode 会返回一行结果，所以用 query_row 而不是 execute。
    let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    let in_memory = conn.path().is_none_or(str::is_empty);
    if !in_memory && !mode.eq_ignore_ascii_case("wal") {
        return Err(StoreError::Invalid(format!(
            "数据库拿不到 WAL 日志模式（得到 {mode}），拒绝打开"
        )));
    }
    conn.execute_batch(&format!(
        "PRAGMA synchronous=FULL;
         PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=5000;
         PRAGMA checkpoint_fullfsync=ON;
         PRAGMA fullfsync={};
         PRAGMA journal_size_limit={};",
        if FULLFSYNC { "ON" } else { "OFF" },
        64 * 1024 * 1024
    ))?;
    Ok(())
}

pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Unix 毫秒（crewd 里唯一读墙钟的地方之一；collab 不读时钟，由调用方传入）。
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// 毫秒 → RFC 3339（毫秒、`Z`），与 `now_ts()` 同格式，可按字符串比较先后。
pub fn ts_from_ms(ms: i64) -> String {
    chrono::DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn now_ts() -> String {
    ts_from_ms(now_ms())
}

/// 只认 UNIQUE（2067）与 PRIMARY KEY（1555）两种扩展码；
/// 外键、NOT NULL、CHECK 等其他约束失败不算“已存在”。
pub fn is_unique_violation(e: &rusqlite::Error) -> bool {
    use rusqlite::ffi::{SQLITE_CONSTRAINT_PRIMARYKEY, SQLITE_CONSTRAINT_UNIQUE};
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _)
            if f.extended_code == SQLITE_CONSTRAINT_UNIQUE
                || f.extended_code == SQLITE_CONSTRAINT_PRIMARYKEY
    )
}

/// 把一列 JSON 文本解成类型；失败转成 rusqlite 的列转换错误，便于在 query_map 里用 `?`。
pub(crate) fn json_col<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    idx: usize,
) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_proto::{Crew, Role, Session};
    use serde_json::json;

    fn crew(id: &str, n: Option<i64>) -> Crew {
        Crew {
            id: id.into(),
            title: format!("机组 {id}"),
            number: n,
            builtin: false,
            parent_ids: vec![],
            working_directory: Some("/tmp/w".into()),
            captain_executor: Some("claude".into()),
            hidden_at: None,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        }
    }

    #[test]
    fn fresh_store_is_empty_and_versioned() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.is_empty().unwrap());
        let v: i64 = s
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, schema::SCHEMA_VERSION);
    }

    #[test]
    fn reopening_file_store_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("crewd.db");
        {
            let s = Store::open(&p).unwrap();
            crews::upsert(s.conn(), &crew("a", Some(1)), None).unwrap();
        }
        let s = Store::open(&p).unwrap();
        assert_eq!(crews::list(s.conn()).unwrap().len(), 1);
        assert!(!s.is_empty().unwrap());
    }

    /// S23：落盘相关的 pragma 写死并读回（不靠编译期默认值；默认值换了版本也不会悄悄变）。
    #[test]
    fn file_store_pins_its_durability_pragmas() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("crewd.db")).unwrap();
        let c = s.conn();
        let text = |sql: &str| c.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap();
        let int = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(text("PRAGMA journal_mode"), "wal");
        assert_eq!(int("PRAGMA synchronous"), 2, "FULL");
        assert_eq!(int("PRAGMA foreign_keys"), 1);
        assert_eq!(int("PRAGMA busy_timeout"), 5000);
        assert_eq!(int("PRAGMA checkpoint_fullfsync"), 1);
        assert_eq!(int("PRAGMA fullfsync"), FULLFSYNC as i64);
        assert_eq!(int("PRAGMA journal_size_limit"), 64 * 1024 * 1024);
    }

    #[test]
    fn newer_schema_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("crewd.db");
        {
            let c = rusqlite::Connection::open(&p).unwrap();
            c.execute_batch("PRAGMA user_version = 99").unwrap();
        }
        let err = Store::open(&p).err().unwrap();
        assert!(err.to_string().contains("99"));
    }

    #[test]
    fn meta_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(meta_get(s.conn(), "k").unwrap(), None);
        meta_set(s.conn(), "k", "v1").unwrap();
        meta_set(s.conn(), "k", "v2").unwrap();
        assert_eq!(meta_get(s.conn(), "k").unwrap().as_deref(), Some("v2"));
    }

    #[test]
    fn crews_list_includes_parents_and_extra_is_kept() {
        let s = Store::open_in_memory().unwrap();
        crews::upsert(
            s.conn(),
            &crew("a", Some(2)),
            Some(&json!({"runtimeLocation":"local_host"})),
        )
        .unwrap();
        crews::upsert(s.conn(), &crew("b", Some(1)), None).unwrap();
        crews::set_parents(s.conn(), "b", &["a".to_string()]).unwrap();
        let list = crews::list(s.conn()).unwrap();
        assert_eq!(
            list.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert_eq!(list[0].parent_ids, vec!["a".to_string()]);
        let extra: String = s
            .conn()
            .query_row("SELECT extra FROM crews WHERE id='a'", [], |r| r.get(0))
            .unwrap();
        assert!(extra.contains("local_host"));
        assert!(crews::exists(s.conn(), "a").unwrap());
        assert!(!crews::exists(s.conn(), "zz").unwrap());
    }

    #[test]
    fn with_tx_rolls_back_on_error() {
        let mut s = Store::open_in_memory().unwrap();
        let r: Result<()> = s.with_tx(|tx| {
            crews::upsert(tx, &crew("a", None), None)?;
            Err(StoreError::Invalid("boom".into()))
        });
        assert!(r.is_err());
        assert!(s.is_empty().unwrap());
    }

    #[test]
    fn sessions_upsert_and_list() {
        let s = Store::open_in_memory().unwrap();
        crews::upsert(s.conn(), &crew("a", None), None).unwrap();
        let mut ses = Session {
            id: "captain-1".into(),
            crew_id: "a".into(),
            role: Role::Captain,
            executor: "claude".into(),
            agent_session_id: Some("uuid-1".into()),
            display_name: Some("机长".into()),
            working_directory: None,
            model: None,
            effort: None,
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        };
        sessions::upsert(s.conn(), &ses, None).unwrap();
        ses.display_name = Some("新机长".into());
        sessions::upsert(s.conn(), &ses, None).unwrap();
        let list = sessions::list_for_crew(s.conn(), "a").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].display_name.as_deref(), Some("新机长"));
    }

    #[test]
    fn crew_and_session_v3_columns_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        let mut c = crew("crew-a", Some(1));
        c.title_source = Some("captain".into());
        c.status_line = Some("示例状态".into());
        c.policy = Some(json!({"mode":"ask","n":2}));
        crews::upsert(s.conn(), &c, None).unwrap();
        let ses = Session {
            id: "w-a1".into(),
            crew_id: "crew-a".into(),
            role: Role::Worker,
            executor: "claude".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            title: Some("示例标题".into()),
            brief: Some("示例任务".into()),
            desired: crewd_proto::Desired::Running,
            ext: Some(3),
            worktree: Some("/tmp/wt".into()),
            was_working: true,
            last_active_at: Some("2026-10-01T00:00:01.000Z".into()),
            last_output_at: Some("2026-10-01T00:00:02.000Z".into()),
            last_progress_at: Some("2026-10-01T00:00:03.000Z".into()),
            awaiting_question: Some("示例问题".into()),
            resume_failed_at: Some("2026-10-01T00:00:04.000Z".into()),
            ..Default::default()
        };
        sessions::upsert(s.conn(), &ses, None).unwrap();
        // 机长指针有外键：session 在了才能指过去。upsert 写它，但从不写 position（只由回填 / collab 写）。
        s.conn()
            .execute("UPDATE crews SET position=7 WHERE id='crew-a'", [])
            .unwrap();
        c.captain_session_id = Some("w-a1".into());
        c.position = 99;
        crews::upsert(s.conn(), &c, None).unwrap();
        let got = crews::get(s.conn(), "crew-a").unwrap().unwrap();
        assert_eq!(got.captain_session_id.as_deref(), Some("w-a1"));
        assert_eq!(got.title_source.as_deref(), Some("captain"));
        assert_eq!(got.status_line.as_deref(), Some("示例状态"));
        assert_eq!(got.policy, Some(json!({"mode":"ask","n":2})));
        assert_eq!(got.position, 7, "upsert 不动 position");
        assert!(crews::get(s.conn(), "nope").unwrap().is_none());
        assert_eq!(sessions::get(s.conn(), "w-a1").unwrap().unwrap(), ses);
        assert!(sessions::get(s.conn(), "nope").unwrap().is_none());
        assert_eq!(sessions::list_all(s.conn()).unwrap(), vec![ses.clone()]);
        assert_eq!(
            sessions::list_for_crew(s.conn(), "crew-a").unwrap(),
            vec![ses]
        );
    }

    #[test]
    fn unknown_stored_vocabulary_is_a_read_error_not_a_default() {
        let s = Store::open_in_memory().unwrap();
        crews::upsert(s.conn(), &crew("crew-a", None), None).unwrap();
        s.conn()
            .execute(
                "INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('w-x','crew-a','chief','claude','t')",
                [],
            )
            .unwrap();
        assert!(sessions::get(s.conn(), "w-x").is_err());
        assert!(sessions::list_all(s.conn()).is_err());
    }

    #[test]
    fn agent_session_id_ledger() {
        let s = Store::open_in_memory().unwrap();
        crews::upsert(s.conn(), &crew("a", None), None).unwrap();
        let ses = Session {
            id: "w-1".into(),
            crew_id: "a".into(),
            role: Role::Worker,
            executor: "claude".into(),
            agent_session_id: None,
            display_name: None,
            working_directory: None,
            model: None,
            effort: None,
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        };
        sessions::upsert(s.conn(), &ses, None).unwrap();
        assert_eq!(sessions::agent_session_id(s.conn(), "w-1").unwrap(), None);
        sessions::set_agent_session_id(s.conn(), "w-1", "uuid-9").unwrap();
        assert_eq!(
            sessions::agent_session_id(s.conn(), "w-1")
                .unwrap()
                .as_deref(),
            Some("uuid-9")
        );
        let list = sessions::list_for_crew(s.conn(), "a").unwrap();
        assert_ne!(
            list[0].updated_at, "2026-10-01T00:00:00.000Z",
            "落账顺带更新时间"
        );
        assert!(matches!(
            sessions::agent_session_id(s.conn(), "nope"),
            Err(StoreError::NotFound(_))
        ));
        assert!(matches!(
            sessions::set_agent_session_id(s.conn(), "nope", "x"),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn is_unique_violation_only_matches_unique_and_primary_key() {
        let s = Store::open_in_memory().unwrap();
        crews::upsert(s.conn(), &crew("a", None), None).unwrap();

        // PRIMARY KEY 冲突
        let pk = s
            .conn()
            .execute(
                "INSERT INTO crews(id,title,builtin,created_at,updated_at) VALUES('a','t',0,'x','x')",
                [],
            )
            .unwrap_err();
        assert!(is_unique_violation(&pk));

        // UNIQUE 冲突（messages.id 是 UNIQUE 列）
        channels::ensure_lobby(s.conn(), "a", "x").unwrap();
        let ins = "INSERT INTO messages(id,channel_id,sender_kind,text,created_at) VALUES('m1','a/lobby','human','t','x')";
        s.conn().execute(ins, []).unwrap();
        let uq = s.conn().execute(ins, []).unwrap_err();
        assert!(is_unique_violation(&uq));

        // FOREIGN KEY 失败不算
        let fk = match crews::set_parents(s.conn(), "a", &["nope".to_string()]) {
            Err(StoreError::Sql(e)) => e,
            other => panic!("expected sql error, got {other:?}"),
        };
        assert!(!is_unique_violation(&fk));

        // NOT NULL 失败不算
        let nn = s
            .conn()
            .execute("INSERT INTO crews(id) VALUES('z')", [])
            .unwrap_err();
        assert!(!is_unique_violation(&nn));
    }

    #[test]
    fn ts_from_ms_roundtrips() {
        assert_eq!(ts_from_ms(0), "1970-01-01T00:00:00.000Z");
        // 计划原写 13:33:20；`date -u -r 1790000000` 实测 14:13:20（控制者裁定 R14）。
        assert_eq!(ts_from_ms(1_790_000_000_123), "2026-09-21T14:13:20.123Z");
    }

    #[test]
    fn now_ts_is_utc_millis() {
        let t = now_ts();
        assert_eq!(t.len(), "2026-10-05T00:00:00.000Z".len());
        assert!(t.ends_with('Z'));
    }
}
