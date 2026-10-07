//! crewd 的协作层（spec §22.2）：群、成员、发消息、游标、机组 DAG、人类待确认、唤醒队列。
//! 同步、不碰 tokio、不读时钟：每个写操作是 `fn(&mut Tx, ..) -> Result<T>`，时间是 `Tx` 里调用方给的
//! `now_ms`；操作把「改了什么」记成 `Change`，`run` 提交成功才交回（服务层据此发事件）。
//! M3b / M3c 只经这里写协作数据（spec §22.5）。
pub mod asks;
pub mod channels;
pub mod crews;
pub mod cursors;
pub mod directory;
pub mod hibernate;
pub mod listens;
pub mod messages;
pub mod prompts;
pub mod refusal;
pub mod render;
pub mod restart;
pub mod sessions;
pub mod settings;
#[cfg(test)]
mod testkit;
pub mod todos;
pub mod turn_trace;
pub mod wakes;
pub mod worldview;

use crate::refusal::Refusal;
use crewd_proto::RestartAnswer;
use crewd_store::{Store, StoreError};
use rusqlite::Connection;

/// 一次成功提交改了什么。服务层把它变成客户端事件，并原样发到内部通道（`crewd_server::hub::CollabEvent`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    MessageAppended {
        channel_id: String,
        seq: i64,
        message_id: String,
        /// 发消息的 session（人 / 系统发的是 None）。M3c 的熔断判「有没有新产出」会用。
        sender_session_id: Option<String>,
    },
    ChannelChanged {
        channel_id: String,
    },
    CrewChanged {
        crew_id: String,
    },
    /// 机组被永久删除（purge；客户端事件 `crew.purged`）。移除 = 归档，发的是 `CrewChanged`。
    CrewRemoved {
        crew_id: String,
    },
    /// 群随机组的永久删除被级联删掉了（客户端事件 `channel.purged`；删了的群读不到，`ChannelChanged` 发不出）。
    ChannelRemoved {
        channel_id: String,
    },
    BadgeChanged {
        channel_id: String,
    },
    SessionChanged {
        session_id: String,
    },
    AskChanged {
        crew_id: String,
        ask: String,
    },
    /// 人回答了一条问（裁定 17）：`cleared` = 卡点被清的 (工作项, 负责人)，M4b 据此叫醒负责人。
    AskAnswered {
        crew_id: String,
        ask: String,
        asked_by: Option<String>,
        cleared: Vec<(String, Option<String>)>,
    },
    WakeEnqueued {
        wake_id: i64,
        session_id: String,
    },
    RestartChanged {
        id: i64,
    },
    /// 人答了重启询问（§22.7 Q1）：M3c 订阅它，`Resume` 时错开逐个 `--resume`。
    RestartAnswered {
        id: i64,
        answer: RestartAnswer,
        session_ids: Vec<String>,
    },
    /// 这个 session 有了产出（`sessions::note_progress`，控制者裁定 C3）：只走内部通道，M3c 的环路熔断消费。
    Progress {
        session_id: String,
        kind: ProgressKind,
    },
}

/// 环路熔断认的「产出」（§22.7 Q5、控制者裁定 C3）。发言不是产出：回话给叫醒你的人不该让环路续命。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProgressKind {
    /// 调了一个真正干活的工具（M3b 的 post-tool-use；crewd 自己的通讯类工具不报）。
    ToolCall,
    /// 名下工作项有变化（M4b 的工作项命令必须报）。
    WorkItem,
}

#[derive(Debug, thiserror::Error)]
pub enum CollabError {
    #[error("{0}")]
    NotFound(Refusal),
    #[error("{0}")]
    Invalid(Refusal),
    #[error("{0}")]
    Forbidden(Refusal),
    #[error("{0}")]
    Conflict(Refusal),
    #[error("{0}")]
    Work(crewd_work::Rejected),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl CollabError {
    /// 规则拒绝（四个变体）带的码 + 参数；客户端靠 `code.as_str()` 本地化。`Work` / `Store` 没有码。
    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            CollabError::NotFound(r)
            | CollabError::Invalid(r)
            | CollabError::Forbidden(r)
            | CollabError::Conflict(r) => Some(r),
            CollabError::Work(_) | CollabError::Store(_) => None,
        }
    }
}

impl From<rusqlite::Error> for CollabError {
    fn from(e: rusqlite::Error) -> Self {
        // 经 StoreError 的 From 归类：磁盘满 / I/O / 损坏 / 只读 / 打不开不落成普通 SQL 错（S26）。
        CollabError::Store(StoreError::from(e))
    }
}

impl From<serde_json::Error> for CollabError {
    fn from(e: serde_json::Error) -> Self {
        CollabError::Store(StoreError::Json(e))
    }
}

pub type Result<T> = std::result::Result<T, CollabError>;

/// 渲染语言（`settings::locale`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    Zh,
    En,
}

impl Locale {
    pub fn parse(s: &str) -> Option<Locale> {
        match s {
            "zh" => Some(Locale::Zh),
            "en" => Some(Locale::En),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Locale::Zh => "zh",
            Locale::En => "en",
        }
    }
}

/// 谁在做这次操作（发消息、建群、改群）。机长 / worker 由 collab 按机长指针现查。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sender {
    Human,
    Session(String),
    /// crewd 自己（公告、系统消息）。
    System,
}

impl Sender {
    pub fn member(&self) -> Option<crewd_proto::MemberRef> {
        match self {
            Sender::Human => Some(crewd_proto::MemberRef::Human),
            Sender::Session(id) => Some(crewd_proto::MemberRef::session(id.clone())),
            Sender::System => None,
        }
    }
}

/// 一个 SQLite 事务 + 调用方给的时间 + 这次产生的变更。
pub struct Tx<'a> {
    conn: &'a Connection,
    now_ms: i64,
    changes: Vec<Change>,
}

impl<'a> Tx<'a> {
    pub fn conn(&self) -> &'a Connection {
        self.conn
    }
    pub fn now_ms(&self) -> i64 {
        self.now_ms
    }
    pub fn now_ts(&self) -> String {
        crewd_store::ts_from_ms(self.now_ms)
    }
    /// 记一条变更（同一条只记一次）。
    pub fn note(&mut self, c: Change) {
        if !self.changes.contains(&c) {
            self.changes.push(c);
        }
    }
}

/// 在一个 SQLite 事务里跑 `f`；成功才提交并交回变更，失败整体回滚、变更作废。
pub fn run<T>(
    store: &mut Store,
    now_ms: i64,
    f: impl FnOnce(&mut Tx<'_>) -> Result<T>,
) -> Result<(T, Vec<Change>)> {
    let t = store.conn().unchecked_transaction()?;
    let (v, changes) = {
        let mut tx = Tx {
            conn: &t,
            now_ms,
            changes: Vec::new(),
        };
        let v = f(&mut tx)?;
        (v, tx.changes)
    };
    t.commit()?;
    Ok((v, changes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, count, store};

    /// S26：collab 里 `?` 过来的 rusqlite 错误也要按存储故障归类，不能一律落成 `Sql`。
    #[test]
    fn sqlite_storage_failures_keep_their_class_through_collab() {
        let full = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
            None,
        );
        let e = CollabError::from(full);
        assert!(
            matches!(&e, CollabError::Store(s) if s.storage_kind() == Some(crewd_store::StorageKind::DiskFull)),
            "{e:?}"
        );
    }

    #[test]
    fn run_commits_and_hands_back_changes_once() {
        let mut s = store();
        let (v, changes) = run(&mut s, NOW, |tx| {
            tx.conn()
                .execute("INSERT INTO meta(key,value) VALUES('k','v')", [])?;
            tx.note(Change::CrewChanged {
                crew_id: "crew-a".into(),
            });
            tx.note(Change::CrewChanged {
                crew_id: "crew-a".into(),
            });
            Ok(tx.now_ms())
        })
        .unwrap();
        assert_eq!(v, NOW);
        assert_eq!(
            changes,
            [Change::CrewChanged {
                crew_id: "crew-a".into()
            }],
            "同一变更只记一次"
        );
        assert_eq!(count(&s, "SELECT COUNT(*) FROM meta WHERE key='k'"), 1);
    }

    #[test]
    fn an_error_rolls_back_and_drops_changes() {
        let mut s = store();
        let r: Result<()> = run(&mut s, NOW, |tx| {
            tx.conn()
                .execute("INSERT INTO meta(key,value) VALUES('k','v')", [])?;
            tx.note(Change::CrewChanged {
                crew_id: "crew-a".into(),
            });
            Err(CollabError::Invalid(Refusal::new(
                crate::refusal::Code::MessageEmpty,
            )))
        })
        .map(|(v, _)| v);
        assert!(matches!(r, Err(CollabError::Invalid(_))));
        assert_eq!(count(&s, "SELECT COUNT(*) FROM meta WHERE key='k'"), 0);
    }

    #[test]
    fn now_ts_is_the_callers_time() {
        let mut s = store();
        let (ts, _) = run(&mut s, 0, |tx| Ok(tx.now_ts())).unwrap();
        assert_eq!(ts, "1970-01-01T00:00:00.000Z");
    }
}
