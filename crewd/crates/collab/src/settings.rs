//! crewd 的设置（存 `meta`）。§22.7 Q5：并发上限默认不设；语言默认 zh（见计划「人的拍板」末段）。
use crate::refusal::{Code, Refusal};
use crate::{CollabError, Locale, Result, Tx};
use rusqlite::Connection;

const LOCALE_KEY: &str = "locale";
const MAX_LIVE_KEY: &str = "maxLiveSessions";
const HIBERNATE_DEADLINE_KEY: &str = "hibernate.deadlineSecs";

pub fn locale(conn: &Connection) -> Result<Locale> {
    match crewd_store::meta_get(conn, LOCALE_KEY)? {
        None => Ok(Locale::default()),
        Some(v) => Locale::parse(&v).ok_or_else(|| {
            CollabError::Invalid(Refusal::new(Code::SettingsBadLocale).arg("value", v.clone()))
        }),
    }
}

pub fn set_locale(tx: &mut Tx<'_>, l: Locale) -> Result<()> {
    crewd_store::meta_set(tx.conn(), LOCALE_KEY, l.as_str())?;
    Ok(())
}

/// 全机同时在跑的 session 上限；None = 不设（§22.7 Q5 默认）。它只挡「拉起会话」（M3c）。
pub fn max_live_sessions(conn: &Connection) -> Result<Option<u32>> {
    match crewd_store::meta_get(conn, MAX_LIVE_KEY)? {
        None => Ok(None),
        Some(v) => v
            .parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .map(Some)
            .ok_or_else(|| {
                CollabError::Invalid(Refusal::new(Code::SettingsBadMaxLive).arg("value", v.clone()))
            }),
    }
}

pub fn set_max_live_sessions(tx: &mut Tx<'_>, n: Option<u32>) -> Result<()> {
    match n {
        None => {
            tx.conn()
                .execute("DELETE FROM meta WHERE key=?1", [MAX_LIVE_KEY])?;
        }
        Some(0) => {
            return Err(CollabError::Invalid(Refusal::new(Code::SettingsMaxLiveMin)));
        }
        Some(n) => crewd_store::meta_set(tx.conn(), MAX_LIVE_KEY, &n.to_string())?,
    }
    Ok(())
}

/// 休眠（C26）时等在干活的 session 到达安全点的期限（秒）；None = 没设（M3c 用默认值）；0 = 立即休眠。
pub fn hibernate_deadline_secs(conn: &Connection) -> Result<Option<u32>> {
    match crewd_store::meta_get(conn, HIBERNATE_DEADLINE_KEY)? {
        None => Ok(None),
        Some(v) => v.parse::<u32>().map(Some).map_err(|_| {
            CollabError::Invalid(format!("meta.hibernate.deadlineSecs = {v:?} 不是非负整数"))
        }),
    }
}

/// None = 回到默认值。
pub fn set_hibernate_deadline_secs(tx: &mut Tx<'_>, secs: Option<u32>) -> Result<()> {
    match secs {
        None => {
            tx.conn()
                .execute("DELETE FROM meta WHERE key=?1", [HIBERNATE_DEADLINE_KEY])?;
        }
        Some(n) => crewd_store::meta_set(tx.conn(), HIBERNATE_DEADLINE_KEY, &n.to_string())?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{err, ok, store};

    #[test]
    fn defaults_are_zh_and_no_session_cap() {
        let s = store();
        assert_eq!(locale(s.conn()).unwrap(), Locale::Zh);
        assert_eq!(max_live_sessions(s.conn()).unwrap(), None);
    }

    #[test]
    fn session_cap_is_optional_and_positive() {
        let mut s = store();
        ok(&mut s, |tx| set_max_live_sessions(tx, Some(10)));
        assert_eq!(max_live_sessions(s.conn()).unwrap(), Some(10));
        ok(&mut s, |tx| set_max_live_sessions(tx, None));
        assert_eq!(max_live_sessions(s.conn()).unwrap(), None);
        assert!(matches!(
            err(&mut s, |tx| set_max_live_sessions(tx, Some(0))),
            CollabError::Invalid(_)
        ));
    }

    /// C26：休眠期限的设置。没设 = None（默认值 120 秒由 M3c 用 crewd-wake 的常量补）；0 = 立即休眠，是合法值。
    #[test]
    fn hibernate_deadline_is_optional_zero_is_allowed_and_garbage_is_an_error() {
        let mut s = store();
        assert_eq!(hibernate_deadline_secs(s.conn()).unwrap(), None);
        ok(&mut s, |tx| set_hibernate_deadline_secs(tx, Some(0)));
        assert_eq!(hibernate_deadline_secs(s.conn()).unwrap(), Some(0));
        ok(&mut s, |tx| set_hibernate_deadline_secs(tx, Some(300)));
        assert_eq!(hibernate_deadline_secs(s.conn()).unwrap(), Some(300));
        ok(&mut s, |tx| set_hibernate_deadline_secs(tx, None));
        assert_eq!(hibernate_deadline_secs(s.conn()).unwrap(), None);
        crewd_store::meta_set(s.conn(), "hibernate.deadlineSecs", "-1").unwrap();
        assert!(matches!(
            hibernate_deadline_secs(s.conn()),
            Err(CollabError::Invalid(_))
        ));
    }

    #[test]
    fn locale_roundtrips_and_garbage_is_an_error() {
        let mut s = store();
        ok(&mut s, |tx| set_locale(tx, Locale::En));
        assert_eq!(locale(s.conn()).unwrap(), Locale::En);
        crewd_store::meta_set(s.conn(), "locale", "fr").unwrap();
        assert!(locale(s.conn()).is_err(), "认不出不猜");
    }
}
