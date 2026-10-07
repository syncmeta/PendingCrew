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
    /// 这一轮的结局（评审 I3）：开始时在干活的先记 `Pending`，到了安全点改 `Finished`、到期限被打断改 `Interrupted`；
    /// 休眠中途 crewd 没了（电脑先关了）就停在 `Pending`——按被打断算（[`Hibernated::interrupted`]），不能说「做完了」。
    #[serde(default)]
    pub outcome: Outcome,
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

/// 一个休眠停下的 session 的这一轮怎么样了。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// 还在等它到达安全点（休眠中途 crewd 没了就一直是它）。
    #[default]
    Pending,
    /// 在安全点停下的（空闲的、期限内做完这一轮的）。
    Finished,
    /// 到了期限还没做完、被打断了。
    Interrupted,
}

impl Hibernated {
    /// 恢复后要不要当成「被打断」（被打断的命令要重跑）：明确被打断的，或开始时在干活、却没等到结局的（I3）。
    pub fn interrupted(&self) -> bool {
        self.outcome == Outcome::Interrupted
            || (self.was_working && self.outcome == Outcome::Pending)
    }
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
                outcome: Outcome::Interrupted,
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
