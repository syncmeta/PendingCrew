//! Authenticated SQLite knowledge controller. The Core DB actor serializes ACL
//! changes with each bounded knowledge operation; no untrusted SQL reaches Core.
use super::compile;
use crate::{AppState, connection_scope::ConnectionScope, public_commands::Actor};
use crewd_collab::{self as collab, Sender};
use crewd_proto::{Request, ResourceActor, Response, RpcError, team_data::*};
use crewd_store::team_data::{self as store, Knowledge, Mutation, Stored};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};

pub(crate) struct Runtime {
    root: PathBuf,
}
#[derive(Clone)]
enum Access {
    Human(Actor),
    Session {
        session: String,
        token: String,
        scope: ConnectionScope,
    },
}
impl Access {
    fn human(&self) -> bool {
        matches!(self, Self::Human(_))
    }
    fn check(
        &self,
        state: &Arc<AppState>,
        c: &rusqlite::Connection,
        crew: &str,
        write: bool,
    ) -> collab::Result<ResourceActor> {
        let sender = match self {
            Self::Human(actor) => {
                actor.authorize_tx(
                    state,
                    c,
                    if write {
                        "data.submit"
                    } else {
                        "data.workspace.get"
                    },
                )?;
                Sender::Human
            }
            Self::Session {
                session,
                token,
                scope,
            } => {
                if !scope.is_live() || !state.tokens.verify(session, token) {
                    return Err(crewd_store::StoreError::Invalid(
                        "session token or connection unavailable".into(),
                    )
                    .into());
                }
                Sender::Session(session.clone())
            }
        };
        collab::team_data::authorize(c, &sender, crew, write)
    }
}
enum KnowledgeError {
    Rpc(RpcError),
    Store(crewd_store::StoreError),
    Sql(rusqlite::Error),
    Collab(collab::CollabError),
    Validation(String),
}
impl KnowledgeError {
    fn rpc(self) -> RpcError {
        match self {
            Self::Rpc(e) => e,
            Self::Store(e) => RpcError::new(
                RpcError::CONFLICT,
                format!("knowledge operation refused: {e}"),
            ),
            Self::Sql(_) => {
                RpcError::new(RpcError::CONFLICT, "knowledge SQL unavailable or refused")
            }
            Self::Collab(_) => RpcError::new(RpcError::FORBIDDEN, "knowledge resource unavailable"),
            Self::Validation(s) => RpcError::new(RpcError::CONFLICT, s),
        }
    }
}
impl crewd_store::db_health::DbFaultSource for KnowledgeError {
    fn db_fault_kind(&self) -> Option<crewd_store::db_health::DbFaultKind> {
        match self {
            Self::Store(e) => e.db_fault_kind(),
            Self::Sql(e) => e.db_fault_kind(),
            Self::Collab(e) => e.db_fault_kind(),
            _ => None,
        }
    }
}
impl From<RpcError> for KnowledgeError {
    fn from(e: RpcError) -> Self {
        Self::Rpc(e)
    }
}
impl From<crewd_store::StoreError> for KnowledgeError {
    fn from(e: crewd_store::StoreError) -> Self {
        Self::Store(e)
    }
}
impl From<rusqlite::Error> for KnowledgeError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}
impl From<collab::CollabError> for KnowledgeError {
    fn from(e: collab::CollabError) -> Self {
        Self::Collab(e)
    }
}
impl From<String> for KnowledgeError {
    fn from(e: String) -> Self {
        Self::Validation(e)
    }
}
impl From<&str> for KnowledgeError {
    fn from(e: &str) -> Self {
        Self::Validation(e.into())
    }
}
impl From<std::num::ParseIntError> for KnowledgeError {
    fn from(_: std::num::ParseIntError) -> Self {
        Self::Validation("invalid integer".into())
    }
}
impl From<serde_json::Error> for KnowledgeError {
    fn from(_: serde_json::Error) -> Self {
        Self::Validation("invalid knowledge JSON".into())
    }
}
fn error(e: impl Into<KnowledgeError>) -> KnowledgeError {
    e.into()
}
fn parse<T: DeserializeOwned>(v: Value) -> Result<T, KnowledgeError> {
    crewd_proto::parse_params(v).map_err(Into::into)
}
fn encode(v: impl Serialize) -> Result<Value, KnowledgeError> {
    serde_json::to_value(v).map_err(error)
}
impl Runtime {
    pub(crate) fn new(root: PathBuf) -> crewd_store::Result<Self> {
        let parent = root
            .parent()
            .ok_or_else(|| crewd_store::StoreError::Invalid("knowledge parent missing".into()))?;
        let name = root.file_name().ok_or_else(|| {
            crewd_store::StoreError::Invalid("knowledge directory name missing".into())
        })?;
        // Core already opened this data directory. Normalize its ancestor aliases
        // (such as macOS /tmp) without following a symlink at the knowledge root.
        let root = std::fs::canonicalize(parent)?.join(name);
        store::private_dir(&root)?;
        Ok(Self { root })
    }
    pub(crate) fn qualified(&self) -> bool {
        true
    }
    pub(crate) async fn handle(
        self: &Arc<Self>,
        state: Arc<AppState>,
        actor: Actor,
        request: Request,
    ) -> Response {
        let id = request.id;
        let principal = match actor.principal(&state).await {
            Ok(p) => p,
            Err(e) => return Response::err(id, e),
        };
        match self
            .route(
                state,
                Access::Human(actor),
                principal,
                &request.method,
                request.params,
            )
            .await
        {
            Ok(v) => Response::ok(id, v),
            Err(e) => Response::err(id, e),
        }
    }
    pub(crate) async fn session(
        self: &Arc<Self>,
        state: Arc<AppState>,
        session: String,
        token: String,
        scope: ConnectionScope,
        method: &str,
        args: Value,
    ) -> Result<Value, RpcError> {
        let principal = format!("session:{session}");
        self.route(
            state,
            Access::Session {
                session,
                token,
                scope,
            },
            principal,
            method,
            args,
        )
        .await
    }
    async fn route(
        self: &Arc<Self>,
        state: Arc<AppState>,
        access: Access,
        principal: String,
        method: &str,
        args: Value,
    ) -> Result<Value, RpcError> {
        let method = method.to_owned();
        let root = self.root.clone();
        let state_for = state.clone();
        state
            .db
            .call_result(move |core| {
                route_sync(
                    core.conn(),
                    root,
                    state_for,
                    access,
                    principal,
                    method,
                    args,
                )
            })
            .await
            .map_err(|e| error(e).rpc())?
            .map_err(KnowledgeError::rpc)
    }
}
fn route_sync(
    core: &rusqlite::Connection,
    root: PathBuf,
    state_for: Arc<AppState>,
    access: Access,
    principal: String,
    method: String,
    args: Value,
) -> Result<Value, KnowledgeError> {
    let crew = if method == "data.workspace.get" {
        parse::<DataWorkspaceGetParams>(args.clone())?.crew_id
    } else {
        args.get("context")
            .and_then(|v| v.get("crewId"))
            .and_then(Value::as_str)
            .ok_or_else(|| error("crew context required"))?
            .to_owned()
    };
    let write = matches!(
        method.as_str(),
        "data.submit" | "data.preview" | "data.confirm" | "data.restore"
    );
    let actor = access
        .check(&state_for, core, &crew, write)
        .map_err(error)?;
    let db_id = crewd_store::meta_get(core, "dbId")
        .map_err(error)?
        .ok_or_else(|| error("Core database identity missing"))?;
    let ns = store::namespace(&db_id, &crew);
    let base = DataContext {
        db_id: db_id.clone(),
        authority_id: db_id,
        restore_epoch: "initial".into(),
        crew_id: crew,
        namespace: ns.clone(),
        generation: "0".into(),
    };
    let path = root.join(format!("{ns}.sqlite"));
    if !path.exists() && method == "data.workspace.get" {
        return encode(DataWorkspaceInfo {
            context: base,
            state: "uninitialized".into(),
            engine_version: "SQLite".into(),
            reason: None,
        });
    }
    if !access.human()
        && matches!(
            method.as_str(),
            "data.confirm" | "data.restore" | "data.snapshots" | "data.preview.get"
        )
    {
        return Err(error("real Human recovery authority required"));
    }
    let mut knowledge = Knowledge::open(&path, base, write).map_err(error)?;
    let commit_access = access.clone();
    let commit_state = state_for.clone();
    knowledge.before_commit = Some(Box::new(move || {
        let live = match &commit_access {
            Access::Human(actor) => actor.connection_scope().is_live(),
            Access::Session {
                session,
                token,
                scope,
            } => scope.is_live() && commit_state.tokens.verify(session, token),
        };
        if !live {
            return Err(crewd_store::StoreError::Invalid(
                "original knowledge connection or token closed before commit".into(),
            ));
        }
        Ok(())
    }));
    let context = knowledge.context().map_err(error)?;
    let workspace = || DataWorkspaceInfo {
        context: context.clone(),
        state: "ready".into(),
        engine_version: rusqlite::version().into(),
        reason: None,
    };
    let result = match method.as_str() {
        "data.workspace.get" => encode(workspace()),
        "data.request.get" => {
            let p: DataRequestGetParams = parse(args)?;
            let lookup = knowledge
                .lookup(&p.request_id, &p.context, &principal, access.human())
                .map_err(error)?;
            encode(DataRequestGetResult {
                workspace: workspace(),
                lookup,
            })
        }
        "data.submit" => {
            let p: DataSubmitParams = parse(args)?;
            let digest = store::digest(&p).map_err(error)?;
            if let Some(result) = knowledge
                .retry(&p.request_id, &p.context, &principal, &digest)
                .map_err(error)?
            {
                return encode(result);
            }
            knowledge.check(&p.context).map_err(error)?;
            match &p.change {
                DataChange::Apply { message, edits } => {
                    if message.len() > 2048 {
                        return Err(error("change message exceeds bound"));
                    }
                    let allocations = compile::allocations(&p.context, edits).map_err(error)?;
                    let created = knowledge.allocate(&allocations).map_err(error)?;
                    let port = Port {
                        knowledge: &knowledge,
                    };
                    let mutations = compile::compile(
                        &p.context,
                        &created,
                        &actor,
                        &crewd_store::now_ts(),
                        edits,
                        &port,
                    )
                    .map_err(error)?;
                    encode(
                        knowledge
                            .apply(
                                &p.request_id,
                                &p.context,
                                &principal,
                                &digest,
                                created,
                                mutations,
                                false,
                                None,
                            )
                            .map_err(error)?,
                    )
                }
                DataChange::RestoreObject {
                    reference,
                    revision,
                    expected_revision,
                } => {
                    let current = knowledge.get(reference, None).map_err(error)?;
                    if store::object_fields(&current.object).2 != expected_revision {
                        return Err(error("object revision conflict"));
                    }
                    let mut old = knowledge.get(reference, Some(revision)).map_err(error)?;
                    let next = expected_revision
                        .parse::<u64>()
                        .map_err(error)?
                        .checked_add(1)
                        .ok_or_else(|| error("revision exhausted"))?
                        .to_string();
                    match &mut old.object {
                        DataObject::Block {
                            revision,
                            provenance,
                            ..
                        }
                        | DataObject::View {
                            revision,
                            provenance,
                            ..
                        }
                        | DataObject::Relation {
                            revision,
                            provenance,
                            ..
                        } => {
                            *revision = next;
                            provenance.updated_by = actor;
                            provenance.updated_at = crewd_store::now_ts();
                        }
                    }
                    encode(
                        knowledge
                            .apply(
                                &p.request_id,
                                &p.context,
                                &principal,
                                &digest,
                                Default::default(),
                                vec![Mutation::PutObject {
                                    object: old.object,
                                    expected_revision: Some(expected_revision.clone()),
                                    captures: old.captures,
                                }],
                                false,
                                None,
                            )
                            .map_err(error)?,
                    )
                }
            }
        }
        "data.object.get" => {
            let p: DataObjectGetParams = parse(args)?;
            knowledge.check(&p.context).map_err(error)?;
            let stored = knowledge
                .get(&p.reference, p.at_revision.as_deref())
                .map_err(error)?;
            let (object, references) =
                project(&knowledge, stored, p.at_revision.is_some()).map_err(error)?;
            let mut affected = Vec::new();
            let mut after = None;
            let mut truncated = false;
            loop {
                let (items, next) = knowledge
                    .list("view", after.as_deref(), 100, false)
                    .map_err(error)?;
                for view in items {
                    let (_, id, _, _) = store::object_fields(&view);
                    let r = DataRef {
                        namespace: context.namespace.clone(),
                        kind: "view".into(),
                        id: id.into(),
                    };
                    let v = knowledge.get(&r, None).map_err(error)?;
                    if v.captures
                        .iter()
                        .any(|c| c.live && c.id == p.reference.id && c.kind == p.reference.kind)
                    {
                        affected.push(r);
                    }
                }
                if affected.len() >= 100 {
                    affected.truncate(100);
                    truncated = true;
                    break;
                }
                if next.is_none() {
                    break;
                }
                after = next;
            }
            encode(DataObjectDetail {
                context: p.context,
                resolved_context: context.clone(),
                generation: context.generation,
                object,
                references,
                affected_live_views: affected,
                affected_truncated: truncated,
            })
        }
        "data.object.list" => {
            let p: DataObjectListParams = parse(args)?;
            knowledge.check(&p.context).map_err(error)?;
            if p.at_revision.is_some() {
                return Err(error("object revision is only valid for a specific object"));
            }
            let (items, next) = knowledge
                .list(
                    &p.object_type,
                    p.after.as_deref(),
                    p.limit,
                    p.include_archived,
                )
                .map_err(error)?;
            encode(DataObjectPage {
                context: p.context,
                generation: context.generation,
                items,
                next,
            })
        }
        "data.history" => {
            let p: DataHistoryParams = parse(args)?;
            knowledge.check(&p.context).map_err(error)?;
            let (items, next) = knowledge
                .history(&p.reference, p.after.as_deref(), p.limit)
                .map_err(error)?;
            encode(DataHistory {
                context: p.context,
                items,
                next,
            })
        }
        "data.query" => {
            let p: DataQueryParams = parse(args)?;
            knowledge.check(&p.context).map_err(error)?;
            if p.at_revision.is_some() || p.cursor.is_some() {
                return Err(error(
                    "historical SQL and implicit SQL cursors are not supported; use bounded current queries",
                ));
            }
            let (columns, rows, truncated) =
                knowledge.query(&p.statement, p.limit).map_err(error)?;
            encode(DataRows {
                columns,
                rows,
                truncated,
                next: None,
                generation: context.generation,
                stable_order: false,
            })
        }
        "data.schema.get" => {
            let p: DataSchemaParams = parse(args)?;
            knowledge.check(&p.context).map_err(error)?;
            if p.at_revision.is_some() {
                return Err(error("historical SQL schema is not supported"));
            }
            crewd_store::team_data::bound(p.limit, 100).map_err(error)?;
            let mut q=knowledge.conn.prepare("SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE '_pc_%' AND name NOT LIKE 'sqlite_%' AND (?1 IS NULL OR name>?1) ORDER BY name LIMIT ?2").map_err(error)?;
            let rows = q
                .query_map(rusqlite::params![p.after, p.limit + 1], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(error)?;
            let next = if rows.len() > p.limit as usize {
                Some(rows[p.limit as usize - 1].1.clone())
            } else {
                None
            };
            let objects = rows
                .into_iter()
                .take(p.limit as usize)
                .map(|(kind, name, sql)| DataSchemaObject {
                    kind,
                    name,
                    create_sql: sql.unwrap_or_default(),
                    columns: vec![],
                    attributes: Value::Null,
                })
                .collect();
            encode(DataSchemaResult {
                context: p.context,
                generation: context.generation,
                objects,
                next,
            })
        }
        "data.table.get" => {
            let p: DataTableParams = parse(args)?;
            knowledge.check(&p.context).map_err(error)?;
            if p.at_revision.is_some() || p.after_key.is_some() {
                return Err(error("historical or implicit table cursors not supported"));
            }
            if p.table.starts_with('_')
                || !p
                    .table
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(error("invalid business table identifier"));
            }
            let (columns, rows, truncated) = knowledge
                .query(
                    &DataSql {
                        sql: format!(
                            "SELECT * FROM \"{}\" LIMIT {}",
                            p.table,
                            p.limit.saturating_add(1)
                        ),
                        params: vec![],
                    },
                    p.limit,
                )
                .map_err(error)?;
            encode(DataTablePage {
                context: p.context,
                generation: context.generation,
                columns,
                rows,
                next_key: None,
                truncated,
                stable_order: false,
            })
        }
        "data.preview.get" => encode(knowledge.preview_get(parse(args)?).map_err(error)?),
        "data.preview" => encode(knowledge.preview(parse(args)?, &principal).map_err(error)?),
        "data.confirm" => encode(knowledge.confirm(parse(args)?, &principal).map_err(error)?),
        "data.restore" => encode(knowledge.restore(parse(args)?, &principal).map_err(error)?),
        "data.snapshots" => {
            let p: DataSnapshotsParams = parse(args)?;
            knowledge.check(&p.context).map_err(error)?;
            encode(DataSnapshots {
                context: p.context,
                items: knowledge.snapshots().map_err(error)?,
            })
        }
        _ => Err(RpcError::new(RpcError::METHOD_NOT_FOUND, "knowledge method unavailable").into()),
    };
    if matches!(
        method.as_str(),
        "data.submit" | "data.confirm" | "data.restore"
    ) && result.as_ref().is_ok_and(|v| v["phase"] == "committed")
    {
        state_for.hub.emit(
            core,
            &[collab::Change::DataChanged {
                crew_id: context.crew_id.clone(),
            }],
        );
    }
    result
}
struct Port<'a> {
    knowledge: &'a Knowledge,
}
impl compile::Port for Port<'_> {
    fn object(&self, r: &DataRef, at: Option<&str>) -> Result<compile::Stored, String> {
        let v = self.knowledge.get(r, at).map_err(|e| e.to_string())?;
        Ok(compile::Stored {
            object: v.object,
            captures: v.captures,
        })
    }
    fn target(&self, r: &DataRef, at: Option<&str>) -> Result<compile::Target, String> {
        let v = self.knowledge.get(r, at).map_err(|e| e.to_string())?;
        Ok(compile::Target {
            commit: Some(
                self.knowledge
                    .context()
                    .map_err(|e| e.to_string())?
                    .generation,
            ),
            revision: Some(store::object_fields(&v.object).2.into()),
        })
    }
}
fn project(
    k: &Knowledge,
    mut stored: Stored,
    historical: bool,
) -> crewd_store::Result<(DataObject, Vec<DataResolvedRef>)> {
    let mut refs = Vec::new();
    for capture in &stored.captures {
        let reference = DataRef {
            namespace: capture.namespace.clone(),
            kind: capture.kind.clone(),
            id: capture.id.clone(),
        };
        let pinned = historical || !capture.live;
        let target = k.get(
            &reference,
            if pinned {
                Some(&capture.revision)
            } else {
                None
            },
        );
        let (target, revision, complete) = match target {
            Ok(s) => {
                let (_, _, r, archived) = store::object_fields(&s.object);
                if archived && !pinned {
                    (DataRefState::Unavailable, None, false)
                } else {
                    (
                        DataRefState::Visible { reference },
                        Some(r.to_owned()),
                        true,
                    )
                }
            }
            Err(_) => (DataRefState::Unavailable, None, false),
        };
        refs.push(DataResolvedRef {
            anchor: capture.anchor.clone(),
            target: target.clone(),
            commit: None,
            revision: revision.clone(),
            live: !pinned,
            complete,
        });
        match &mut stored.object {
            DataObject::View {
                definition: DataViewDefinition::Document { placements, .. },
                ..
            } => {
                for p in placements
                    .iter_mut()
                    .filter(|p| capture.anchor == format!("placement:{}", p.id))
                {
                    p.block = target.clone();
                    if pinned {
                        p.at_revision = revision.clone();
                    }
                }
            }
            DataObject::View {
                definition: DataViewDefinition::Web { bindings, .. },
                ..
            } => {
                for b in bindings
                    .iter_mut()
                    .filter(|b| capture.anchor == format!("binding:{}", b.id))
                {
                    if let DataBindingSource::Object {
                        reference,
                        at_revision,
                    } = &mut b.source
                    {
                        *reference = target.clone();
                        if pinned {
                            *at_revision = revision.clone();
                        }
                    }
                }
            }
            DataObject::Relation {
                subject, object, ..
            } => {
                if capture.anchor == "relation:subject" {
                    *subject = target.clone();
                }
                if capture.anchor == "relation:object" {
                    *object = target.clone();
                }
            }
            _ => {}
        }
        let provenance = match &mut stored.object {
            DataObject::Block { provenance, .. }
            | DataObject::View { provenance, .. }
            | DataObject::Relation { provenance, .. } => provenance,
        };
        if let Some(n) = capture
            .anchor
            .strip_prefix("source:")
            .and_then(|n| n.parse::<usize>().ok())
        {
            if let Some(s) = provenance.sources.get_mut(n) {
                s.reference = Some(target);
                if pinned {
                    s.at_revision = revision;
                }
            }
        }
    }
    // Historical SQL cannot silently evaluate against today's business tables.
    if historical {
        if let DataObject::View {
            definition: DataViewDefinition::Web { bindings, .. },
            ..
        } = &mut stored.object
        {
            bindings.retain(|b| {
                !matches!(
                    b.source,
                    DataBindingSource::Query { .. } | DataBindingSource::Objects { .. }
                )
            });
        }
    }
    Ok((stored.object, refs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testkit::Fixture;
    use crewd_proto::agent::methods;
    use serde_json::json;
    #[test]
    fn sqlite_runtime_accepts_parent_alias_but_rejects_unsafe_knowledge_root() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temporary = tempfile::tempdir().unwrap();
        let parent = std::fs::canonicalize(temporary.path()).unwrap();
        let data = parent.join("data");
        std::fs::create_dir(&data).unwrap();
        let alias = parent.join("data-alias");
        symlink(&data, &alias).unwrap();

        let runtime = Runtime::new(alias.join("knowledge")).unwrap();
        assert_eq!(runtime.root, data.join("knowledge"));
        std::fs::remove_dir(&runtime.root).unwrap();

        let elsewhere = parent.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&elsewhere, data.join("knowledge")).unwrap();
        assert!(Runtime::new(alias.join("knowledge")).is_err());
        std::fs::remove_file(data.join("knowledge")).unwrap();

        std::fs::create_dir(data.join("knowledge")).unwrap();
        std::fs::set_permissions(
            data.join("knowledge"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(Runtime::new(alias.join("knowledge")).is_err());
    }

    async fn runtime(f: &Fixture) -> Arc<Runtime> {
        let root = std::fs::canonicalize(f.data_dir())
            .unwrap()
            .join("knowledge");
        let rt = Arc::new(Runtime::new(root).unwrap());
        assert!(f.state.data_runtime.set(rt.clone()).is_ok());
        rt
    }
    fn human() -> Actor {
        let policy = Arc::new(crate::peer::PeerPolicy::new(None).unwrap());
        let (server, _client) = tokio::net::UnixStream::pair().unwrap();
        Actor::Local(policy.accept(&server).unwrap())
    }
    async fn agent(rt: &Arc<Runtime>, f: &Fixture, method: &str, args: Value) -> Value {
        rt.session(
            f.state.clone(),
            f.captain.clone(),
            f.token(&f.captain),
            ConnectionScope::new(),
            method,
            args,
        )
        .await
        .unwrap()
    }
    /// Preparation-only unless the owner authorizes one real, offline SDK import.
    /// No turn, tool invocation, login, or version request is available on this client.
    #[tokio::test]
    #[ignore = "real SDK import only: requires explicit CREWD_CODEX_INVENTORY_ONCE=1 authorization"]
    async fn real_codex_imported_knowledge_inventory_without_turn() {
        use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        assert_eq!(
            std::env::var("CREWD_CODEX_INVENTORY_ONCE").as_deref(),
            Ok("1")
        );
        let root = PathBuf::from("/private/tmp/pcw-codex-import-inventory-20261017-01");
        assert!(
            root.is_dir(),
            "owner must create the reviewed private root first"
        );
        assert!(
            !root.join("sdk-started.json").exists(),
            "single attempt only"
        );
        let sdk = PathBuf::from(
            "/Users/hey/.codex/packages/standalone/releases/0.162.1-aarch64-apple-darwin/bin/codex",
        );
        let helper = PathBuf::from(
            "/private/tmp/pcw-mac-daily-889a2e62-mac-02/PendingCrew.app/Contents/Helpers/crewd",
        );
        fn sha(path: &std::path::Path) -> String {
            ring::digest::digest(&ring::digest::SHA256, &std::fs::read(path).unwrap())
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        }
        assert_eq!(
            sha(&sdk),
            "74c6a6d263f40d65f99c27a815275f5b4ed0bcc2baecc396b4cf3255498bdfc0"
        );
        assert_eq!(
            sha(&helper),
            "618e920e5d402534f24e62caa04ef808fc429567cac4ec9d50174ebf8ee5abc5"
        );
        let f = Fixture::new().await;
        assert!(
            f.data_dir().starts_with(root.join("tmp")),
            "isolated TMPDIR required"
        );
        let _rt = runtime(&f).await;
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let socket_task = f.spawn_owned_socket(stop_rx).await;
        let socket = f.data_dir().join("crewd.sock");
        let token_file = root.join("agent-token");
        let mut grant = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&token_file)
            .unwrap();
        std::io::Write::write_all(&mut grant, f.token(&f.worker).as_bytes()).unwrap();
        drop(grant);
        let quote = |v: &std::path::Path| serde_json::to_string(v.to_str().unwrap()).unwrap();
        // All IP networking is denied. The sole socket exception is this fixture.
        // The account HOME is denied, except public executable resources.
        let profile = format!(
            "(version 1)\n(allow default)\n(deny network*)\n(allow network-outbound (remote unix-socket (literal {})))\n(deny file-read* file-write* (subpath \"/Users/hey\"))\n(allow file-read* (subpath \"/Users/hey/.codex/packages/standalone/releases/0.162.1-aarch64-apple-darwin\"))\n(deny mach-lookup (global-name \"com.apple.securityd\"))\n",
            quote(&socket)
        );
        let sandbox_file = root.join("offline.sb");
        std::fs::write(&sandbox_file, profile).unwrap();
        // Syntax was checked separately with /usr/bin/true; this does not run SDK discovery.
        let mut command = tokio::process::Command::new("/usr/bin/sandbox-exec");
        command.args([
            "-f".as_ref(),
            sandbox_file.as_os_str(),
            sdk.as_os_str(),
            "app-server".as_ref(),
        ]);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .env("CODEX_HOME", root.join("codex-home"))
            .env("TMPDIR", root.join("tmp"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("SHELL", "/usr/bin/false")
            .current_dir(root.join("work"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        command.as_std_mut().process_group(0);
        // The marker remains even if startup fails: no alternate API or second SDK attempt.
        std::fs::write(
            root.join("sdk-started.json"),
            json!({"sdk":sdk,"helper":helper,"noTurns":true}).to_string(),
        )
        .unwrap();
        let spawned = command.spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                f.state.tokens.revoke(&f.worker);
                let _ = std::fs::remove_file(&token_file);
                let _ = stop_tx.send(());
                let _ = socket_task.await;
                panic!("single SDK startup failed, no retry: {error}");
            }
        };
        let mut input = child.stdin.take().unwrap();
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let mut stderr = child.stderr.take().unwrap();
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            use tokio::io::AsyncReadExt;
            let _ = (&mut stderr).take(1_048_576).read_to_end(&mut bytes).await;
            bytes
        });
        let mut frames = Vec::new();
        async fn send(
            input: &mut tokio::process::ChildStdin,
            frames: &mut Vec<Value>,
            packet: Value,
        ) -> anyhow::Result<()> {
            let method = packet["method"].as_str().unwrap_or_default();
            anyhow::ensure!(
                matches!(
                    method,
                    "initialize" | "initialized" | "thread/start" | "mcpServerStatus/list"
                ),
                "outgoing method is outside the no-model allowlist"
            );
            frames.push(packet.clone());
            input.write_all(format!("{packet}\n").as_bytes()).await?;
            Ok(())
        }
        async fn receive(
            lines: &mut tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
            frames: &mut Vec<Value>,
            id: u64,
        ) -> anyhow::Result<Value> {
            loop {
                let line =
                    tokio::time::timeout(std::time::Duration::from_secs(20), lines.next_line())
                        .await??
                        .ok_or_else(|| anyhow::anyhow!("SDK EOF"))?;
                anyhow::ensure!(line.len() <= 4_194_304, "SDK frame bound exceeded");
                let packet: Value = serde_json::from_str(&line)?;
                frames.push(packet.clone());
                anyhow::ensure!(
                    !(packet.get("method").is_some() && packet.get("id").is_some()),
                    "SDK requested an unexpected action; stop without approval"
                );
                if packet["id"] == id {
                    anyhow::ensure!(
                        packet.get("error").is_none(),
                        "SDK unsupported/auth/error: {packet}"
                    );
                    return Ok(packet["result"].clone());
                }
            }
        }
        let outcome: anyhow::Result<Value> = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            async {
            send(&mut input, &mut frames, json!({"id":1,"method":"initialize","params":crewd_exec::codex::proto::initialize_params("inventory-once")})).await?;
            receive(&mut lines, &mut frames, 1).await?;
            send(&mut input, &mut frames, json!({"method":"initialized"})).await?;
            let options = crewd_exec::codex::proto::ThreadOptions {
                mcp_servers: Some(crewd_exec::codex::proto::mcp_servers(&helper, &[
                    "mcp".into(), "--data-dir".into(), f.data_dir().display().to_string(),
                    "--session".into(), f.worker.clone(), "--token-file".into(), token_file.display().to_string(),
                ])),
                ..Default::default()
            };
            send(&mut input, &mut frames, json!({"id":2,"method":"thread/start","params":crewd_exec::codex::proto::thread_start_params(&root.join("work"), &options)})).await?;
            let thread = receive(&mut lines, &mut frames, 2).await?;
            let thread_id = thread["thread"]["id"].as_str().ok_or_else(|| anyhow::anyhow!("thread id absent"))?;
            let mut cursor = Value::Null;
            let mut pages = Vec::new();
            for index in 0..4u64 {
                let id = 10 + index;
                send(&mut input, &mut frames, json!({"id":id,"method":"mcpServerStatus/list","params":{"threadId":thread_id,"serverName":"crew","detail":"toolsAndAuthOnly","cursor":cursor,"limit":100}})).await?;
                let page = receive(&mut lines, &mut frames, id).await?;
                cursor = page.get("nextCursor").cloned().unwrap_or(Value::Null);
                pages.push(page);
                if cursor.is_null() { return Ok(json!({"pages":pages,"threadId":thread_id})); }
            }
            anyhow::bail!("bounded inventory exceeded four pages; no alternative query")
            },
        )
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("fixed 120-second SDK protocol deadline exceeded; no retry")));
        drop(input); // EOF is the normal SDK shutdown; no model or tool call was sent.
        let exit =
            match tokio::time::timeout(std::time::Duration::from_secs(10), child.wait()).await {
                Ok(exit) => format!("{exit:?}"),
                Err(_) => {
                    let _ = child.start_kill(); // Only the exact held new SDK child.
                    format!("owned SDK EOF timeout; kill/wait {:?}", child.wait().await)
                }
            };
        f.state.tokens.revoke(&f.worker);
        let _ = std::fs::remove_file(&token_file);
        let _ = stop_tx.send(());
        let _ = socket_task.await;
        let stderr = tokio::time::timeout(std::time::Duration::from_secs(2), stderr_task).await;
        let _ = std::fs::write(
            root.join("sdk-frames.json"),
            serde_json::to_vec_pretty(&frames).unwrap(),
        );
        if let Ok(Ok(bytes)) = stderr {
            let _ = std::fs::write(root.join("sdk-stderr.txt"), bytes);
        }
        let result = match outcome {
            Ok(inventory) => {
                json!({"scope":"real SDK + formal 889 helper + test-owned production handler/ACL","inventory":inventory,"sdkExit":exit,"turnStartSent":false,"originalAccountPolicyReproduced":false})
            }
            Err(error) => {
                json!({"scope":"single offline SDK inventory attempt","stopped":error.to_string(),"sdkExit":exit,"turnStartSent":false,"retryAllowed":false})
            }
        };
        std::fs::write(
            root.join("result.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn sqlite_formal_mcp_acl_free_view_and_cross_language_wire() {
        let f = Fixture::new().await;
        let rt = runtime(&f).await;
        let socket = f.spawn_socket().await;
        let mut client = crate::agent::testkit::RpcClient::connect(&socket).await;
        client
            .call(
                methods::ATTACH,
                json!({"sessionId":f.captain,"token":f.token(&f.captain)}),
            )
            .await
            .unwrap();
        let tools = client.call(methods::TOOLS, json!({})).await.unwrap();
        assert!(
            tools
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "data_submit")
        );
        assert!(
            !tools
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "data_branches")
        );
        let mut observer = crate::agent::testkit::RpcClient::connect(&socket).await;
        let hello = observer.call("system.hello", json!({})).await.unwrap();
        assert!(
            hello["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "data")
        );
        observer
            .call("events.subscribe", json!({"topics":["data"]}))
            .await
            .unwrap();
        let workspace = agent(&rt, &f, "data.workspace.get", json!({"crewId":f.crew})).await;
        let context = workspace["context"].clone();
        let request = uuid::Uuid::new_v4().to_string();
        let submit = json!({"requestId":request,"context":context,"change":{"kind":"apply","message":"free view fixture","edits":[
            {"kind":"createBlock","alias":"shared","contentType":"text/plain","content":"durable shared block","sources":[],"attributes":{}},
            {"kind":"createView","alias":"canvas","title":"Agent generated workspace","definition":{"kind":"web","renderer":"web","rendererVersion":"1","html":"<article><button id='change'>Change</button><main id='content'></main></article>","css":"article { color: navy }","javascript":"let shared; knowledge.request('read',{binding:'shared'}).then(({response})=>{shared=response.value.object;document.querySelector('#content').textContent=shared.content;}); document.querySelector('#change').onclick=async()=>{if(!shared)return;const result=await knowledge.request('write',{message:'Update shared block',edits:[{kind:'updateBlock',id:shared.id,expectedRevision:shared.revision,contentType:shared.contentType,content:'Agent generated',sources:null,attributes:{}}]});document.querySelector('#content').textContent=result.response.status;};","bindings":[{"kind":"set","id":"shared","source":{"kind":"object","reference":{"kind":"created","alias":"shared"},"atRevision":null},"attributes":{}}],"requestedCapabilities":["knowledge-write"],"attributes":{}},"sources":[],"attributes":{}},
            {"kind":"createView","alias":"document","title":"Shared document","definition":{"kind":"document","placements":[{"kind":"set","id":null,"alias":"position","block":{"kind":"created","alias":"shared"},"atRevision":null,"layout":{}}],"layout":{}},"sources":[],"attributes":{}}
        ]}});
        client
            .call(
                methods::ATTACH,
                json!({"sessionId":f.captain,"token":f.token(&f.captain)}),
            )
            .await
            .unwrap();
        let called = client
            .call(methods::CALL, json!({"tool":"data_submit","args":submit}))
            .await
            .unwrap();
        // The formal MCP envelope carries the actual typed serialized result.
        let text = called["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{called}"));
        let result: Value = serde_json::from_str(text).unwrap();
        assert_eq!(result["phase"], "committed");
        let event = observer
            .next_event(std::time::Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(event.event, "data.changed");
        assert_eq!(event.data, json!({"crewId":f.crew}));
        let latest = result["context"].clone();
        let view = result["created"]["canvas"]["object"]["reference"].clone();
        let detail = agent(
            &rt,
            &f,
            "data.object.get",
            json!({"context":latest,"reference":view,"atRevision":null}),
        )
        .await;
        assert_eq!(detail["object"]["definition"]["kind"], "web");
        let retry = agent(&rt, &f, "data.submit", submit.clone()).await;
        assert_eq!(retry, result);
        let worker_result = rt
            .session(
                f.state.clone(),
                f.worker.clone(),
                f.token(&f.worker),
                ConnectionScope::new(),
                "data.request.get",
                json!({"requestId":request,"context":context}),
            )
            .await;
        assert!(worker_result.is_err());
        let admin = human();
        let lookup = rt
            .route(
                f.state.clone(),
                Access::Human(admin.clone()),
                admin.principal(&f.state).await.unwrap(),
                "data.request.get",
                json!({"requestId":request,"context":context}),
            )
            .await
            .unwrap();
        assert_eq!(lookup["lookup"]["status"], "known");
        let schema_args = json!({"context":latest,"atRevision":null,"after":null,"limit":100});
        let schema = agent(&rt, &f, "data.schema.get", schema_args).await;
        assert!(schema["objects"].as_array().unwrap().is_empty());
        // Actual serde values, not handwritten wire samples, for the Swift decoder.
        let preview=agent(&rt,&f,"data.preview",json!({"context":latest,"statements":[{"sql":"CREATE TABLE example(id INTEGER PRIMARY KEY)","params":[]}]})).await;
        let principal = admin.principal(&f.state).await.unwrap();
        let confirmed=rt.route(f.state.clone(),Access::Human(admin.clone()),principal.clone(),"data.confirm",json!({"requestId":uuid::Uuid::new_v4().to_string(),"context":latest,"token":preview["token"]})).await.unwrap();
        let snapshots = rt
            .route(
                f.state.clone(),
                Access::Human(admin),
                principal,
                "data.snapshots",
                json!({"context":confirmed["context"]}),
            )
            .await
            .unwrap();
        let fixture = json!({"workspace":workspace,"submit":result,"objectDetail":detail,"preview":preview,"snapshots":snapshots});
        std::fs::write(
            "/private/tmp/pcw-sqlite-serde-wire-20261016.json",
            serde_json::to_vec_pretty(&fixture).unwrap(),
        )
        .unwrap();
        let expired = f.token(&f.captain);
        f.state.tokens.revoke(&f.captain);
        assert!(
            rt.session(
                f.state.clone(),
                f.captain.clone(),
                expired,
                ConnectionScope::new(),
                "data.workspace.get",
                json!({"crewId":f.crew})
            )
            .await
            .is_err()
        );
    }
}
