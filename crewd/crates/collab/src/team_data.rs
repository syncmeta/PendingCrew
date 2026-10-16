//! Reuses the original crew membership and archived-scope authority.
use crate::{Result, Sender, resource_acl};
use crewd_proto::ResourceActor;
use rusqlite::Connection;
pub fn authorize(c: &Connection, by: &Sender, crew: &str, write: bool) -> Result<ResourceActor> {
    resource_acl::actor(c, by, crew, write)
}
