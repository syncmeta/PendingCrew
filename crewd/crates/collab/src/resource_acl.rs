//! Trusted membership/scope decisions shared by documents and view classifications.
use crate::refusal::{Code, Refusal};
use crate::{CollabError, Result, Sender, crews, sessions};
use crewd_proto::ResourceActor;
use rusqlite::Connection;
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
