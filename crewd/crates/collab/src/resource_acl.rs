//! Trusted membership/scope decisions shared by documents and view classifications.
use crate::refusal::{Code, Refusal};
use crate::{CollabError, Result, Sender, crews, sessions};
use crewd_proto::ResourceActor;
use rusqlite::Connection;
pub(crate) struct ReadPlan {
    pub crew: Option<String>,
    session: Option<String>,
}
impl ReadPlan {
    pub(crate) fn of(conn: &Connection, by: &Sender) -> Result<Self> {
        match by {
            Sender::Human => Ok(Self {
                crew: None,
                session: None,
            }),
            Sender::Session(id) => {
                let s = sessions::get(conn, id)?;
                Ok(Self {
                    crew: if crews::is_management(conn, &s.crew_id)? {
                        None
                    } else {
                        Some(s.crew_id)
                    },
                    session: Some(id.clone()),
                })
            }
            Sender::System => Err(CollabError::Forbidden(Refusal::new(
                Code::ResourceForbidden,
            ))),
        }
    }
    pub(crate) fn sql(&self, owner: &str, start: usize) -> (String, Vec<rusqlite::types::Value>) {
        assert!(
            owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
        );
        match &self.crew {
            Some(crew) => (
                format!(
                    "({owner}=?{start} OR EXISTS(SELECT 1 FROM crews acl_crew WHERE acl_crew.id={owner} AND acl_crew.captain_session_id=?{}))",
                    start + 1
                ),
                vec![
                    crew.clone().into(),
                    self.session.clone().unwrap_or_default().into(),
                ],
            ),
            None => ("1".into(), vec![]),
        }
    }
    fn can_read(&self, c: &Connection, crew: &str) -> Result<bool> {
        let (predicate, mut args) = self.sql("acl_target.id", 1);
        args.push(crew.to_string().into());
        Ok(c.query_row(
            &format!(
                "SELECT {predicate} FROM crews acl_target WHERE acl_target.id=?{}",
                args.len()
            ),
            rusqlite::params_from_iter(args),
            |r| r.get(0),
        )?)
    }
}
pub fn invalid(field: &str) -> CollabError {
    CollabError::Invalid(Refusal::new(Code::ResourceInvalid).arg("field", field))
}
pub fn uuid(raw: &str) -> Result<String> {
    uuid::Uuid::parse_str(raw)
        .map(|v| v.to_string())
        .map_err(|_| invalid("id"))
}
pub fn actor(conn: &Connection, by: &Sender, crew: &str, write: bool) -> Result<ResourceActor> {
    crews::get(conn, crew)?;
    if !write && !ReadPlan::of(conn, by)?.can_read(conn, crew)? {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::ResourceForbidden,
        )));
    }
    if write {
        crews::refuse_archived(conn, crew)?;
    }
    match by {
        Sender::Human => Ok(ResourceActor {
            kind: "human".into(),
            session_id: None,
        }),
        Sender::Session(id) => {
            let s = sessions::get(conn, id)?;
            if write {
                crews::refuse_archived(conn, &s.crew_id)?;
            }
            if s.crew_id != crew
                && !crews::acts_as_captain_of(conn, id, crew)?
                && !crews::is_management_session(conn, id)?
            {
                return Err(CollabError::Forbidden(Refusal::new(
                    Code::ResourceForbidden,
                )));
            }
            Ok(ResourceActor {
                kind: "session".into(),
                session_id: Some(id.clone()),
            })
        }
        Sender::System => Err(CollabError::Forbidden(Refusal::new(
            Code::ResourceForbidden,
        ))),
    }
}
pub fn digest<T: serde::Serialize>(p: &T) -> Result<Vec<u8>> {
    let raw = serde_json::to_vec(p)
        .map_err(|_| crewd_store::StoreError::Invalid("resource witness serialization".into()))?;
    Ok(ring::digest::digest(&ring::digest::SHA256, &raw)
        .as_ref()
        .to_vec())
}
