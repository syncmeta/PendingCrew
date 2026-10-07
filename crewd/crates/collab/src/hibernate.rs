//! 休眠记录（C26）：人让 crewd 休眠（`crewd.hibernate`，M3c）时，被停下的每个 session 一项——休眠开始时在不在干活、
//! 到期限有没有被打断、停下时在处理哪批唤醒。存在 `meta`（不加表、不迁移）。下次启动开重启询问时，询问据它标成
//! 「休眠」（`RestartPrompt.reason` / `interrupted`，见 [`crate::restart`]），恢复时 M3c 据它给 agent 附言。
use crate::{CollabError, Result, Tx};
use crewd_proto::RestartReason;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

const KEY: &str = "hibernate";

/// 最近一次休眠（新的一次整份覆盖旧的；M3c 把旧的里还没恢复的项带进新的一份）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// 永远是 `hibernate`（这份记录为什么存在；与重启询问的 `reason` 同一个词）。
    pub reason: RestartReason,
    /// 休眠开始的时刻（Unix 毫秒）。
    pub at_ms: i64,
    /// 下次启动为这些 session 开的那条重启询问（[`crate::restart::open_with`] 填）。
    #[serde(default)]
    pub prompt_id: Option<i64>,
    pub sessions: Vec<Hibernated>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hibernated {
    pub session_id: String,
    /// 休眠开始时它在干活（或正有一批唤醒在投）。
    pub was_working: bool,
    /// 到了期限还没做完那一轮，被打断了（恢复后被打断的命令要重跑）。
    pub interrupted: bool,
    /// 停下之前在处理的那批唤醒的摘要（数据，不是文案；插进附言前过 `safe_name`）。不知道 = None。
    #[serde(default)]
    pub doing: Option<String>,
    /// 那批唤醒的 id。
    #[serde(default)]
    pub last_wakes: Vec<i64>,
    /// 已恢复过（附言给过了）：不再给第二次。项不删，重启询问的 `interrupted` 名单因此不随恢复变化。
    #[serde(default)]
    pub resumed: bool,
}

/// 读不出来（不是这个形状）= `Invalid`：不猜、不当没有。
pub fn get(conn: &Connection) -> Result<Option<Record>> {
    match crewd_store::meta_get(conn, KEY)? {
        None => Ok(None),
        Some(v) => serde_json::from_str(&v)
            .map(Some)
            .map_err(|e| CollabError::Invalid(format!("meta.hibernate 读不出来：{e}"))),
    }
}

/// 整份写入（`reason` 一律写成 `hibernate`）。
pub fn put(tx: &mut Tx<'_>, r: &Record) -> Result<()> {
    let r = Record {
        reason: RestartReason::Hibernate,
        ..r.clone()
    };
    crewd_store::meta_set(tx.conn(), KEY, &serde_json::to_string(&r)?)?;
    Ok(())
}

pub fn clear(tx: &mut Tx<'_>) -> Result<()> {
    tx.conn().execute("DELETE FROM meta WHERE key=?1", [KEY])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{ok, store};

    fn rec() -> Record {
        Record {
            reason: RestartReason::Restart,
            at_ms: 5,
            prompt_id: None,
            sessions: vec![Hibernated {
                session_id: "w-a1".into(),
                was_working: true,
                interrupted: true,
                doing: Some("timer timer:t1: 合成".into()),
                last_wakes: vec![3, 4],
                resumed: false,
            }],
        }
    }

    #[test]
    fn record_round_trips_through_meta_says_hibernate_and_clear_removes_it() {
        let mut s = store();
        assert_eq!(get(s.conn()).unwrap(), None);
        ok(&mut s, |tx| put(tx, &rec()));
        let got = get(s.conn()).unwrap().unwrap();
        assert_eq!(got.reason, RestartReason::Hibernate, "这份记录就是休眠");
        assert_eq!(got.sessions, rec().sessions);
        ok(&mut s, clear);
        assert_eq!(get(s.conn()).unwrap(), None);
    }

    #[test]
    fn a_record_that_does_not_parse_is_an_error_not_nothing() {
        let s = store();
        crewd_store::meta_set(s.conn(), KEY, "{不是").unwrap();
        assert!(matches!(get(s.conn()), Err(CollabError::Invalid(_))));
    }
}
