//! One B15 predicate for point reads, lists and indexed search candidates.
use super::*;
pub(crate) struct ReadPlan {
    crew: String,
    home: String,
    session: String,
    all: bool,
    now: i64,
}
impl ReadPlan {
    pub(crate) fn of(c: &Connection, session: &str, now: i64) -> Result<Self> {
        let crew = sessions::crew_of(c, session)?;
        Ok(Self {
            all: crews::is_management(c, &crew)?,
            home: crews::home_of(c, &crew)?,
            crew,
            session: session.into(),
            now,
        })
    }
    /// Alias is an internal fixed SQL identifier, never user supplied.
    pub(crate) fn sql(&self, alias: &str, start: usize) -> (String, Vec<rusqlite::types::Value>) {
        assert!(
            alias
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        let [all, crew, home, session, now] =
            std::array::from_fn::<_, 5, _>(|i| format!("?{}", start + i));
        let base = format!(
            "({all}=1 OR({alias}.kind='human' AND({alias}.crew_id={crew} OR {alias}.crew_id={home})) OR({alias}.kind!='human' AND({alias}.crew_id={crew} OR EXISTS(SELECT 1 FROM channel_members cm JOIN sessions s ON cm.member_kind='session' AND s.id=cm.member_id WHERE cm.channel_id={alias}.id AND cm.left_at IS NULL AND s.crew_id={crew}))))"
        );
        // An active listen never revives revoked B15 access. Keep the full live
        // qualification explicit so all read surfaces share the same contract.
        let sql = format!(
            "({base} OR({base} AND {alias}.archived_at IS NULL AND EXISTS(SELECT 1 FROM listens l WHERE l.session_id={session} AND l.channel_id={alias}.id AND(l.until IS NULL OR l.until>{now}))))"
        );
        (
            sql,
            vec![
                i64::from(self.all).into(),
                self.crew.clone().into(),
                self.home.clone().into(),
                self.session.clone().into(),
                self.now.into(),
            ],
        )
    }
}
