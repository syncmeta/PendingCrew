//! Authenticated SQLite knowledge controller. The Core DB actor serializes ACL
//! changes with each bounded knowledge operation; no untrusted SQL reaches Core.
use super::compile;
use crate::{AppState, connection_scope::ConnectionScope, public_commands::Actor};
use crewd_collab::{self as collab, Sender};
use crewd_proto::{Request, ResourceActor, Response, RpcError, team_data::*};
use crewd_store::team_data::{self as store, Knowledge, Mutation, Stored};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{cell::OnceCell, path::PathBuf, sync::Arc};

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

fn context_conflict() -> KnowledgeError {
    RpcError::new(
        RpcError::CONFLICT,
        "knowledge generation or restore context changed before submission",
    )
    .with_refusal("data.context_conflict")
    .into()
}
fn parse<T: DeserializeOwned>(v: Value) -> Result<T, KnowledgeError> {
    crewd_proto::parse_params(v).map_err(Into::into)
}
fn encode(v: impl Serialize) -> Result<Value, KnowledgeError> {
    serde_json::to_value(v).map_err(error)
}
impl Runtime {
    pub(crate) fn knowledge_root(&self) -> PathBuf {
        self.root.clone()
    }
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
    // Input-only validation follows current actor/crew authorization, before
    // creating a namespace or attempting any mutation/receipt transaction.
    let submit = if method == "data.submit" {
        let params: DataSubmitParams = parse(args.clone())?;
        if let Some(problem) = params.change.placement_identity_error() {
            return Err(KnowledgeError::Rpc(
                RpcError::new(RpcError::INVALID_PARAMS, problem.message())
                    .with_refusal(problem.refusal()),
            ));
        }
        Some(params)
    } else {
        None
    };
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
            let p = submit.expect("submit was parsed before namespace initialization");
            let digest = store::digest(&p).map_err(error)?;
            if let Some(result) = knowledge
                .retry(&p.request_id, &p.context, &principal, &digest)
                .map_err(error)?
            {
                return encode(result);
            }
            if knowledge.context().map_err(error)? != p.context {
                return Err(context_conflict());
            }
            match &p.change {
                DataChange::Apply { message, edits } => {
                    if message.len() > 2048 {
                        return Err(error("change message exceeds bound"));
                    }
                    let allocations = compile::allocations(&p.context, edits).map_err(error)?;
                    let created = knowledge.allocate(&allocations).map_err(error)?;
                    let port = Port::new(&knowledge, core, &context);
                    let document_work_link = match edits.as_slice() {
                        [DataEdit::CreateRelation { predicate, .. }] => {
                            predicate == compile::DOCUMENT_WORK_PREDICATE
                        }
                        [DataEdit::ArchiveRelation { id, .. }] => knowledge
                            .get(
                                &DataRef {
                                    namespace: context.namespace.clone(),
                                    kind: "relation".into(),
                                    id: id.clone(),
                                },
                                None,
                            )
                            .is_ok_and(|stored| is_document_work_link(&stored.object)),
                        _ => false,
                    };
                    let mutations = compile::compile(
                        &p.context,
                        &created,
                        &actor,
                        &crewd_store::now_ts(),
                        edits,
                        &port,
                    )
                    .map_err(|failure| {
                        if document_work_link {
                            RpcError::new(
                                RpcError::CONFLICT,
                                "document work link rejected before commit",
                            )
                            .with_refusal("data.document_work_link_refused")
                            .into()
                        } else {
                            error(failure)
                        }
                    })?;
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
                    if is_document_work_link(&current.object) || is_document_work_link(&old.object)
                    {
                        return Err(error(
                            "document work links use explicit relation archive changes",
                        ));
                    }
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
                                    object: Box::new(old.object),
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
            let (mut object, mut references) =
                project(&knowledge, stored, p.at_revision.is_some()).map_err(error)?;
            let port = Port::new(&knowledge, core, &context);
            if let Some(reference) =
                port.project_document_work_link(&mut object, p.at_revision.is_some())
            {
                if matches!(
                    object,
                    DataObject::Relation {
                        subject: DataRefState::Unavailable,
                        ..
                    }
                ) && let Some(subject) = references
                    .iter_mut()
                    .find(|r| r.anchor == "relation:subject")
                {
                    subject.target = DataRefState::Unavailable;
                    subject.commit = None;
                    subject.revision = None;
                    subject.complete = false;
                }
                references.push(reference);
            }
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
            let (mut items, next) = knowledge
                .list(
                    &p.object_type,
                    p.after.as_deref(),
                    p.limit,
                    p.include_archived,
                )
                .map_err(error)?;
            let port = Port::new(&knowledge, core, &context);
            for object in &mut items {
                port.project_document_work_link(object, false);
            }
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
            let (mut items, next) = knowledge
                .history(&p.reference, p.after.as_deref(), p.limit)
                .map_err(error)?;
            let port = Port::new(&knowledge, core, &context);
            for object in &mut items {
                port.project_document_work_link(object, true);
            }
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
            let mut q = knowledge
                .conn
                .prepare(
                    "SELECT type,name,sql FROM sqlite_schema
                 WHERE substr(lower(name),1,4) <> '_pc_'
                   AND substr(lower(name),1,7) <> 'sqlite_'
                   AND substr(lower(name),1,7) <> 'pragma_'
                   AND (?1 IS NULL OR name>?1) ORDER BY name LIMIT ?2",
                )
                .map_err(error)?;
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
            let mut next = None;
            let mut bytes = 0;
            let mut objects = Vec::<DataSchemaObject>::new();
            for (kind, name, sql) in rows {
                if objects.len() == p.limit as usize {
                    next = objects.last().map(|row| row.name.clone());
                    break;
                }
                let (columns, attributes) = if matches!(kind.as_str(), "table" | "view") {
                    knowledge.table_schema(&name).map_err(error)?
                } else {
                    (vec![], Value::Null)
                };
                let object = DataSchemaObject {
                    kind,
                    name,
                    create_sql: sql.unwrap_or_default(),
                    columns,
                    attributes,
                };
                bytes += serde_json::to_vec(&object).map_err(error)?.len();
                if bytes > 1024 * 1024 {
                    next = Some(
                        objects
                            .last()
                            .ok_or_else(|| error("SQL schema object exceeds byte bound"))?
                            .name
                            .clone(),
                    );
                    break;
                }
                objects.push(object);
            }
            encode(DataSchemaResult {
                context: p.context,
                generation: context.generation,
                objects,
                next,
            })
        }
        "data.table.get" => {
            let p: DataTableParams = parse(args)?;
            encode(knowledge.table_page(p).map_err(error)?)
        }
        "data.preview.get" => encode(knowledge.preview_get(parse(args)?).map_err(error)?),
        "data.preview" => encode(knowledge.preview(parse(args)?, &principal).map_err(error)?),
        "data.confirm" => {
            let p: DataConfirmParams = parse(args)?;
            let digest = store::digest(&p).map_err(error)?;
            if let Some(result) = knowledge
                .retry(&p.request_id, &p.context, &principal, &digest)
                .map_err(error)?
            {
                return encode(result);
            }
            if context != p.context {
                return Err(context_conflict());
            }
            encode(knowledge.confirm(p, &principal).map_err(error)?)
        }
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
    core: &'a rusqlite::Connection,
    context: &'a DataContext,
    work_book: OnceCell<Option<crewd_work::BookState>>,
}
fn is_document_work_link(object: &DataObject) -> bool {
    matches!(object, DataObject::Relation {
        predicate, object: DataRefState::Visible { reference }, ..
    } if compile::document_work_reference(predicate, reference))
}
impl<'a> Port<'a> {
    fn new(
        knowledge: &'a Knowledge,
        core: &'a rusqlite::Connection,
        context: &'a DataContext,
    ) -> Self {
        Self {
            knowledge,
            core,
            context,
            work_book: OnceCell::new(),
        }
    }
    fn document_active(&self, reference: &DataRef) -> bool {
        reference.namespace == self.context.namespace
            && reference.kind == "view"
            && uuid::Uuid::parse_str(&reference.id).is_ok_and(|id| id.to_string() == reference.id)
            && self.knowledge.get(reference, None).is_ok_and(|stored| {
                matches!(
                    stored.object,
                    DataObject::View {
                        definition: DataViewDefinition::Document { .. },
                        archived: false,
                        ..
                    }
                )
            })
    }
    fn current_work_revision(&self, reference: &DataRef) -> Option<String> {
        use crewd_store::object_refs::{self, ObjectKind};
        if reference.namespace != self.context.namespace {
            return None;
        }
        let kind = match reference.kind.as_str() {
            "plan" => ObjectKind::Plan,
            "requirement" => ObjectKind::Requirement,
            _ => return None,
        };
        let number: i64 = reference.id.strip_prefix(kind.prefix())?.parse().ok()?;
        if number <= 0 {
            return None;
        }
        // Check the real owner's registry before loading any work content.
        let original = object_refs::get(self.core, kind, number).ok()??;
        if original.crew_id != self.context.crew_id || original.id() != reference.id {
            return None;
        }
        // All projection rows share one checked work authority cut under this
        // Core DB worker, rather than rebuilding the whole book for each row.
        use crewd_store::work_organization as organization;
        let book = self
            .work_book
            .get_or_init(|| {
                let book = organization::load(self.core).ok()??;
                organization::verify_references(self.core, &book).ok()?;
                Some(book)
            })
            .as_ref()?;
        let local = organization::core_for_reference(self.core, kind, number).ok()??;
        match kind {
            ObjectKind::Plan => {
                let id: crewd_work::WorkId = local.parse().ok()?;
                if book.item_crew(id).map(crewd_work::CrewId::as_str)
                    != Some(self.context.crew_id.as_str())
                {
                    return None;
                }
                Some(book.items.get(&id)?.revision.to_string())
            }
            ObjectKind::Requirement => {
                let id: crewd_work::EntryNo = local.parse().ok()?;
                if book.entry_crew(id).map(crewd_work::CrewId::as_str)
                    != Some(self.context.crew_id.as_str())
                    || !book.entries.contains_key(&id)
                {
                    return None;
                }
                let source_live: bool = self.core.query_row(
                    "SELECT EXISTS(SELECT 1 FROM object_refs r JOIN todos t ON t.id=r.raw_todo_id
                     WHERE r.kind='requirement' AND r.number=?1 AND t.crew_id=?2
                       AND t.ledger='agent' AND t.deleted_at IS NULL)",
                    rusqlite::params![number,self.context.crew_id], |row| row.get(0),
                ).ok()?;
                if !source_live {
                    return None;
                }
                let object = crewd_proto::ChannelWorkObject {
                    kind: crewd_proto::ChannelWorkKind::Requirement,
                    id: reference.id.clone(),
                };
                collab::todo_attributes::get(self.core, &Sender::Human, &object)
                    .ok()
                    .map(|attrs| attrs.revision)
            }
            _ => None,
        }
    }
    fn project_document_work_link(
        &self,
        relation: &mut DataObject,
        historical: bool,
    ) -> Option<DataResolvedRef> {
        let DataObject::Relation {
            subject,
            predicate,
            object,
            ..
        } = relation
        else {
            return None;
        };
        let DataRefState::Visible { reference } = object else {
            return None;
        };
        if !compile::document_work_reference(predicate, reference) {
            return None;
        }
        if !historical
            && matches!(subject, DataRefState::Visible { reference } if !self.document_active(reference))
        {
            *subject = DataRefState::Unavailable;
        }
        let revision = if historical {
            None
        } else {
            self.current_work_revision(reference)
        };
        let complete = revision.is_some();
        if !complete {
            *object = DataRefState::Unavailable;
        }
        Some(DataResolvedRef {
            anchor: "relation:object".into(),
            target: object.clone(),
            commit: None,
            revision,
            live: !historical,
            complete,
        })
    }
    fn document_work_link_exists(
        &self,
        document: &DataRef,
        work: &DataRef,
        excluding: Option<&str>,
    ) -> rusqlite::Result<bool> {
        self.knowledge.conn.query_row(
            "SELECT EXISTS (
                SELECT 1 FROM _pc_objects
                WHERE kind='relation' AND archived=0 AND (?5 IS NULL OR id<>?5)
                  AND json_extract(body,'$.object.predicate')=?1
                  AND json_extract(body,'$.object.subject.state')='visible'
                  AND json_extract(body,'$.object.subject.reference.kind')='view'
                  AND json_extract(body,'$.object.subject.reference.namespace')=?2
                  AND json_extract(body,'$.object.subject.reference.id')=?3
                  AND json_extract(body,'$.object.object.state')='visible'
                  AND json_extract(body,'$.object.object.reference.namespace')=?2
                  AND json_extract(body,'$.object.object.reference.id')=?4
                  AND json_extract(body,'$.object.object.reference.kind')=?6
            )",
            rusqlite::params![
                compile::DOCUMENT_WORK_PREDICATE,
                self.context.namespace,
                document.id,
                work.id,
                excluding,
                work.kind
            ],
            |row| row.get(0),
        )
    }
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
    fn check_document_work_link(
        &self,
        document: &DataRef,
        work: &DataRef,
        excluding_relation: Option<&str>,
    ) -> Result<(), String> {
        if !self.document_active(document) || self.current_work_revision(work).is_none() {
            return Err("document work link target unavailable".into());
        }
        if self
            .document_work_link_exists(document, work, excluding_relation)
            .map_err(|_| "document work link check unavailable")?
        {
            return Err("document work link already exists".into());
        }
        Ok(())
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
            && let Some(s) = provenance.sources.get_mut(n)
        {
            s.reference = Some(target);
            if pinned {
                s.at_revision = revision;
            }
        }
    }
    // Historical SQL cannot silently evaluate against today's business tables.
    if historical
        && let DataObject::View {
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
    Ok((stored.object, refs))
}

#[cfg(test)]
#[path = "../../../jev/tests/support/mod.rs"]
mod projection_http;

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
    #[tokio::test]
    async fn sqlite_business_schema_and_keyset_use_current_authorized_namespace() {
        let f = Fixture::new().await;
        let rt = runtime(&f).await;
        let workspace = agent(&rt, &f, "data.workspace.get", json!({"crewId":f.crew})).await;
        let old = workspace["context"].clone();
        let submitted = agent(&rt, &f, "data.submit", json!({
            "requestId": uuid::Uuid::new_v4().to_string(),
            "context": old,
            "change":{"kind":"apply","message":"Exact structured fixture","edits":[{"kind":"sql","statements":[
                {"sql":"CREATE TABLE apc_business(id TEXT NOT NULL PRIMARY KEY, qty INTEGER DEFAULT 7, note TEXT) STRICT","params":[]},
                {"sql":"INSERT INTO apc_business VALUES('z', 9, NULL), ('a', 2, 'original'), ('m', 3, NULL)","params":[]},
                {"sql":"CREATE TABLE composite(a TEXT,b TEXT,PRIMARY KEY(a,b)) STRICT","params":[]}
            ]}]}
        })).await;
        assert_eq!(submitted["phase"], "committed");
        let context = submitted["context"].clone();
        let catalog = agent(
            &rt,
            &f,
            "data.schema.get",
            json!({
                "context":context,"atRevision":null,"after":null,"limit":1
            }),
        )
        .await;
        assert_eq!(catalog["objects"].as_array().unwrap().len(), 1);
        assert_eq!(catalog["next"], "apc_business");
        let table = &catalog["objects"][0];
        assert_eq!(table["name"], "apc_business");
        assert_eq!(
            table["attributes"],
            json!({"strict":true,"stablePrimaryKey":true,"nativeRowWrites":true})
        );
        assert_eq!(
            table["columns"][0],
            json!({
                "name":"id","declaredType":"TEXT","notNull":true,"primaryKeyPosition":1,"defaultSql":null,"attributes":null
            })
        );
        assert_eq!(table["columns"][1]["defaultSql"], "7");
        let second = agent(
            &rt,
            &f,
            "data.schema.get",
            json!({
                "context":context,"atRevision":null,"after":catalog["next"],"limit":100
            }),
        )
        .await;
        assert_eq!(second["objects"].as_array().unwrap().len(), 1);
        assert_eq!(second["objects"][0]["name"], "composite");
        assert_eq!(
            second["objects"][0]["attributes"]["stablePrimaryKey"],
            false
        );
        let first = agent(
            &rt,
            &f,
            "data.table.get",
            json!({
                "context":context,"table":"apc_business","atRevision":null,"afterKey":null,"limit":2
            }),
        )
        .await;
        assert_eq!(first["columns"], json!(["id", "qty", "note"]));
        assert_eq!(
            first["rows"],
            json!([
                [{"type":"text","value":"a"},{"type":"integer","value":"2"},{"type":"text","value":"original"}],
                [{"type":"text","value":"m"},{"type":"integer","value":"3"},{"type":"null"}]
            ])
        );
        assert_eq!(first["nextKey"], json!([{"type":"text","value":"m"}]));
        assert_eq!(first["stableOrder"], true);
        let last = agent(&rt, &f, "data.table.get", json!({
            "context":context,"table":"apc_business","atRevision":null,"afterKey":first["nextKey"],"limit":2
        })).await;
        assert_eq!(
            last["rows"],
            json!([[{"type":"text","value":"z"},{"type":"integer","value":"9"},{"type":"null"}]])
        );
        assert!(last["nextKey"].is_null());
        assert_eq!(last["truncated"], false);
        let stale = rt
            .session(
                f.state.clone(),
                f.worker.clone(),
                f.token(&f.worker),
                ConnectionScope::new(),
                "data.table.get",
                json!({"context":old,"table":"apc_business","atRevision":null,
                                   "afterKey":first["nextKey"],"limit":2}),
            )
            .await;
        assert!(stale.is_err());
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
        run_codex_knowledge_boundary(false).await;
    }

    #[tokio::test]
    #[ignore = "local synthetic Responses capture: requires explicit one-shot authorization"]
    async fn real_codex_tools_projection_to_local_capture() {
        run_codex_knowledge_boundary(true).await;
    }

    async fn run_codex_knowledge_boundary(capture: bool) {
        use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let (permission, directory) = if capture {
            (
                "CREWD_CODEX_PROJECTION_ONCE",
                "/private/tmp/pcw-codex-tools-projection-20261017-02",
            )
        } else {
            (
                "CREWD_CODEX_INVENTORY_ONCE",
                "/private/tmp/pcw-codex-import-inventory-20261017-01",
            )
        };
        assert_eq!(std::env::var(permission).as_deref(), Ok("1"));
        let root = PathBuf::from(directory);
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
        // Reserve the reviewed address before creating any grant or SDK process.
        let capture_listener = if capture {
            Some(
                tokio::net::TcpListener::bind("127.0.0.1:39271")
                    .await
                    .expect("fixed local capture endpoint must be free; no fallback"),
            )
        } else {
            None
        };
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
        // Reuse the existing HTTP fixture reader; one connection and one response only.
        // This listener is never a proxy and contains no model or tool executor.
        let (capture_tx, mut capture_rx) = tokio::sync::oneshot::channel();
        let capture_task = if let Some(listener) = capture_listener {
            let output = root.clone();
            Some(tokio::spawn(async move {
                let result: anyhow::Result<Value> = async {
                    let (mut stream, peer) = listener.accept().await?;
                    drop(listener); // Never accept a retry or a second request.
                    anyhow::ensure!(peer.ip().is_loopback(), "non-local capture caller");
                    let request = super::projection_http::read_request(&mut stream).await.ok_or_else(|| anyhow::anyhow!("HTTP request incomplete"))?;
                    let mut headers = json!({"method":request.method,"path":request.path,"contentType":request.header("content-type"),"contentEncoding":request.header("content-encoding"),"contentLength":request.header("content-length"),"transferEncoding":request.header("transfer-encoding"),"bodyBytes":request.body.len(),"authorizationHeaderPresent":request.header("authorization").is_some()});
                    std::fs::write(output.join("capture-http.json"), serde_json::to_vec_pretty(&headers)?)?;
                    std::fs::write(output.join("capture-request-body.bin"), &request.body)?;
                    let response = json!({"type":"response.completed","sequence_number":0,"response":{"id":"resp_local_projection_capture","object":"response","created_at":0,"status":"completed","output":[],"model":"gpt-6.1-sol","usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}});
                    let body = format!("event: response.completed\ndata: {response}\n\n");
                    let head = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                    stream.write_all(head.as_bytes()).await?;
                    stream.write_all(body.as_bytes()).await?;
                    stream.shutdown().await?;
                    anyhow::ensure!(request.method == "POST" && request.path == "/v1/responses", "unexpected first HTTP route; single capture stopped");
                    anyhow::ensure!(request.header("authorization").is_none(), "unexpected authorization header; no account capture allowed");
                    anyhow::ensure!(request.header("transfer-encoding").is_none() && request.header("content-encoding").is_none(), "encoded or chunked body: incomplete projection evidence, no retry");
                    let declared = request.header("content-length").and_then(|value| value.parse::<usize>().ok()).ok_or_else(|| anyhow::anyhow!("missing or invalid Content-Length: incomplete projection evidence"))?;
                    anyhow::ensure!(declared == request.body.len(), "short or oversized body: incomplete projection evidence");
                    let body: Value = serde_json::from_slice(&request.body)?;
                    let tools = body.get("tools").and_then(Value::as_array).ok_or_else(|| anyhow::anyhow!("complete JSON body has no tools array; no projection conclusion"))?;
                    headers["completeJsonToolsArray"] = json!(true);
                    headers["toolEntries"] = json!(tools.len());
                    std::fs::write(output.join("capture-http.json"), serde_json::to_vec_pretty(&headers)?)?;
                    Ok(headers)
                }.await;
                let _ = capture_tx.send(result);
            }))
        } else {
            drop(capture_tx);
            None
        };
        let quote = |v: &std::path::Path| serde_json::to_string(v.to_str().unwrap()).unwrap();
        // All IP networking is denied. The sole socket exception is this fixture.
        // The account HOME is denied, except public executable resources.
        let profile = format!(
            "(version 1)\n(allow default)\n(deny network*)\n(allow network-outbound (remote unix-socket (literal {})))\n(deny file-read* file-write* (subpath \"/Users/hey\"))\n(allow file-read* (subpath \"/Users/hey/.codex/packages/standalone/releases/0.162.1-aarch64-apple-darwin\"))\n(deny mach-lookup (global-name \"com.apple.securityd\"))\n",
            quote(&socket)
        );
        let profile = if capture {
            format!("{profile}(allow network-outbound (remote ip \"localhost:39271\"))\n")
        } else {
            profile
        };
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
            json!({"sdk":sdk,"helper":helper,"noTurns":!capture,"localSyntheticCapture":capture})
                .to_string(),
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
            capture: bool,
            packet: Value,
        ) -> anyhow::Result<()> {
            let method = packet["method"].as_str().unwrap_or_default();
            anyhow::ensure!(
                matches!(
                    method,
                    "initialize" | "initialized" | "thread/start" | "mcpServerStatus/list"
                ) || (capture && method == "turn/start"),
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
            send(&mut input, &mut frames, capture, json!({"id":1,"method":"initialize","params":crewd_exec::codex::proto::initialize_params("inventory-once")})).await?;
            receive(&mut lines, &mut frames, 1).await?;
            send(&mut input, &mut frames, capture, json!({"method":"initialized"})).await?;
            let options = crewd_exec::codex::proto::ThreadOptions {
                model: capture.then(|| "gpt-6.1-sol".to_string()),
                mcp_servers: Some(crewd_exec::codex::proto::mcp_servers(&helper, &[
                    "mcp".into(), "--data-dir".into(), f.data_dir().display().to_string(),
                    "--session".into(), f.worker.clone(), "--token-file".into(), token_file.display().to_string(),
                ])),
                ..Default::default()
            };
            let mut params = crewd_exec::codex::proto::thread_start_params(&root.join("work"), &options);
            if capture {
                params["config"]["model_provider"] = json!("pcw_local_capture");
                params["config"]["model_providers"] = json!({"pcw_local_capture":{"name":"local synthetic capture only","base_url":"http://127.0.0.1:39271/v1","wire_api":"responses","requires_openai_auth":false,"supports_websockets":false,"request_max_retries":0,"stream_max_retries":0}});
            }
            send(&mut input, &mut frames, capture, json!({"id":2,"method":"thread/start","params":params})).await?;
            let thread = receive(&mut lines, &mut frames, 2).await?;
            let thread_id = thread["thread"]["id"].as_str().ok_or_else(|| anyhow::anyhow!("thread id absent"))?;
            let mut cursor = Value::Null;
            let mut pages = Vec::new();
            for index in 0..4u64 {
                let id = 10 + index;
                send(&mut input, &mut frames, capture, json!({"id":id,"method":"mcpServerStatus/list","params":{"threadId":thread_id,"serverName":"crew","detail":"toolsAndAuthOnly","cursor":cursor,"limit":100}})).await?;
                let page = receive(&mut lines, &mut frames, id).await?;
                cursor = page.get("nextCursor").cloned().unwrap_or(Value::Null);
                pages.push(page);
                if cursor.is_null() { break; }
            }
            anyhow::ensure!(cursor.is_null(), "bounded inventory exceeded four pages; no alternative query");
            let inventory = json!({"pages":pages,"threadId":thread_id});
            if capture {
                let ready = pages.iter().flat_map(|p| p["data"].as_array().into_iter().flatten()).any(|server| server["name"] == "crew" && server["runtimeStatus"] == "connected" && server["tools"].get("data_submit").is_some());
                anyhow::ensure!(ready, "crew inventory is not explicitly connected with data_submit; no turn sent");
                send(&mut input, &mut frames, capture, json!({"id":30,"method":"turn/start","params":{"threadId":thread_id,"input":[{"type":"text","text":"Synthetic local tool-definition capture only. No business task or tool execution.","text_elements":[]}]}})).await?;
                receive(&mut lines, &mut frames, 30).await?;
                // Keep stdout live while HTTP is pending, as the production driver does.
                // Requests are evidence only: this control never grants an SDK action.
                let captured = loop {
                    tokio::select! {
                        captured = &mut capture_rx => break captured??,
                        line = lines.next_line() => {
                            let line = line?.ok_or_else(|| anyhow::anyhow!("SDK EOF before HTTP capture"))?;
                            anyhow::ensure!(line.len() <= 4_194_304, "SDK frame bound exceeded");
                            let packet: Value = serde_json::from_str(&line)?;
                            frames.push(packet.clone());
                            anyhow::ensure!(!(packet.get("method").is_some() && packet.get("id").is_some()), "SDK requested an unexpected action; recorded and stopped without approval");
                            anyhow::ensure!(!matches!(packet["method"].as_str(), Some("error" | "turn/completed")), "SDK reported error or turn completion before HTTP capture; recorded and stopped");
                        }
                    }
                };
                return Ok(json!({"inventory":inventory,"capturedRequest":captured,"modelNameSource":"gpt-6.1-sol was the pinned SDK default in the prior isolated inventory; original account model/policy is not reproduced"}));
            }
            Ok(inventory)
            },
        )
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("fixed 120-second SDK protocol deadline exceeded; no retry")));
        drop(input); // EOF: inventory sends no turn; capture uses only its fixed local endpoint.
        let exit =
            match tokio::time::timeout(std::time::Duration::from_secs(10), child.wait()).await {
                Ok(exit) => format!("{exit:?}"),
                Err(_) => {
                    let _ = child.start_kill(); // Only the exact held new SDK child.
                    format!("owned SDK EOF timeout; kill/wait {:?}", child.wait().await)
                }
            };
        if let Some(task) = capture_task {
            task.abort();
            let _ = task.await;
        }
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
                json!({"scope":"real SDK + formal 889 helper + test-owned production handler/ACL","inventory":inventory,"sdkExit":exit,"turnStartSent":capture,"onlyLocalSyntheticResponse":capture,"originalAccountPolicyReproduced":false})
            }
            Err(error) => {
                json!({"scope":"single offline SDK inventory attempt","stopped":error.to_string(),"sdkExit":exit,"turnStartAllowedOnlyAtLocalCapture":capture,"retryAllowed":false})
            }
        };
        std::fs::write(
            root.join("result.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
    }

    /// Real02's original change, without copying its live context, token or UUID.
    /// Only fixture-owned workspace contexts/request IDs are used below.
    fn real02_document_change() -> Value {
        json!({"kind":"apply","message":"保存日用版验收知识及持久关联的简单页面","edits":[
            {"kind":"createBlock","alias":"dailyKnowledge","attributes":{},"content":"日用版知识验收完成","contentType":"text/plain","sources":[]},
            {"kind":"createView","alias":"dailyPage","attributes":{},"definition":{"kind":"document","layout":{},"placements":[{"block":{"alias":"dailyKnowledge","kind":"created"},"kind":"set","layout":{}}]},"sources":[],"title":"日用版知识验收"}
        ]})
    }

    #[tokio::test]
    async fn sqlite_confirm_context_conflict_is_definite_after_original_receipt_retry() {
        let f = Fixture::new().await;
        let rt = runtime(&f).await;
        let admin = human();
        let principal = admin.principal(&f.state).await.unwrap();
        let call = |method: &'static str, args: Value| {
            rt.route(
                f.state.clone(),
                Access::Human(admin.clone()),
                principal.clone(),
                method,
                args,
            )
        };
        let workspace = call("data.workspace.get", json!({"crewId":f.crew}))
            .await
            .unwrap();
        let base = workspace["context"].clone();
        let seeded = call("data.submit", json!({
            "requestId":uuid::Uuid::new_v4().to_string(),"context":base,
            "change":{"kind":"apply","message":"Confirm generation fixture","edits":[{"kind":"sql","statements":[
                {"sql":"CREATE TABLE sample(id INTEGER PRIMARY KEY, value TEXT)","params":[]},
                {"sql":"INSERT INTO sample VALUES(1,'original')","params":[]}
            ]}]}
        })).await.unwrap();
        let context = seeded["context"].clone();
        let make_preview = |value| {
            json!({"context":context,"statements":[{
                "sql":"UPDATE sample SET value=? WHERE id=1","params":[{"type":"text","value":value}]
            }]})
        };
        let first = call("data.preview", make_preview("first")).await.unwrap();
        let competing = call("data.preview", make_preview("stale")).await.unwrap();
        assert_eq!(first["affectedRows"], 1);
        assert_eq!(competing["affectedRows"], 1);
        let packet = |preview: Value| {
            json!({
                "requestId":uuid::Uuid::new_v4().to_string(),"context":context,"token":preview["token"]
            })
        };
        let original = packet(first);
        let stale = packet(competing);
        let receipt = call("data.confirm", original.clone()).await.unwrap();
        let latest = receipt["context"].clone();
        assert_eq!(receipt["phase"], "committed");
        let snapshots = call("data.snapshots", json!({"context":latest}))
            .await
            .unwrap();
        let refusal = call("data.confirm", stale.clone()).await.unwrap_err();
        assert_eq!(refusal.code, RpcError::CONFLICT);
        assert_eq!(refusal.data.unwrap().refusal, "data.context_conflict");
        let absence = call(
            "data.request.get",
            json!({
                "context":context,"requestId":stale["requestId"]
            }),
        )
        .await
        .unwrap();
        assert_eq!(absence["lookup"]["status"], "notFound");
        assert_eq!(call("data.confirm", original).await.unwrap(), receipt);
        assert_eq!(
            call("data.snapshots", json!({"context":latest}))
                .await
                .unwrap(),
            snapshots
        );
        let rows = call("data.query", json!({
            "context":latest,"statement":{"sql":"SELECT value FROM sample WHERE id=1","params":[]},
            "atRevision":null,"cursor":null,"limit":2
        })).await.unwrap();
        assert_eq!(rows["rows"], json!([[{"type":"text","value":"first"}]]));
    }

    #[tokio::test]
    async fn sqlite_native_document_context_conflict_is_typed_after_receipt_retry() {
        let f = Fixture::new().await;
        let rt = runtime(&f).await;
        let call = |method: &str, params| {
            rt.handle(
                f.state.clone(),
                human(),
                Request {
                    id: 1,
                    method: method.into(),
                    params,
                },
            )
        };
        let result = |reply: Response| match reply.outcome {
            crewd_proto::Outcome::Result(value) => value,
            crewd_proto::Outcome::Error(error) => panic!("unexpected RPC refusal: {error:?}"),
        };
        let workspace = result(call("data.workspace.get", json!({"crewId":f.crew})).await);
        let context = workspace["context"].clone();
        let mut change = real02_document_change();
        change["edits"][1]["definition"]["placements"][0]["alias"] = json!("position");
        let original =
            json!({"context":context,"requestId":uuid::Uuid::new_v4().to_string(),"change":change});
        let committed = result(call("data.submit", original.clone()).await);
        assert_eq!(committed["phase"], "committed");
        let known = result(call("data.submit", original.clone()).await);
        assert_eq!(
            known, committed,
            "original UUID still returns the original receipt"
        );

        let mut stale = original;
        stale["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        let crewd_proto::Outcome::Error(refusal) = call("data.submit", stale.clone()).await.outcome
        else {
            panic!("a stale context accepted a new request");
        };
        assert_eq!(refusal.code, RpcError::CONFLICT);
        assert_eq!(refusal.data.unwrap().refusal, "data.context_conflict");
        let lookup = result(
            call(
                "data.request.get",
                json!({"context":context,"requestId":stale["requestId"]}),
            )
            .await,
        );
        assert_eq!(lookup["lookup"]["status"], "notFound");
        assert_eq!(lookup["workspace"]["context"], committed["context"]);
    }

    #[tokio::test]
    async fn sqlite_document_work_links_are_scoped_revisioned_and_readable_after_reopen() {
        let f = Fixture::new().await;
        let rt = runtime(&f).await;
        let socket = f.spawn_socket().await;
        let mut client = crate::agent::testkit::RpcClient::connect(&socket).await;
        let hello = client.call("system.hello", json!({})).await.unwrap();
        assert!(
            hello["capabilities"]
                .as_array()
                .unwrap()
                .contains(&json!("data.documentWorkLinks"))
        );
        let request = |context: &Value, edit: Value| {
            json!({"context":context,"requestId":uuid::Uuid::new_v4().to_string(),
                   "change":{"kind":"apply","message":"document work link fixture","edits":[edit]}})
        };
        let requirement = client.call("todo.create", json!({"crewId":f.crew,"requestId":uuid::Uuid::new_v4().to_string(),"text":"Original request"})).await.unwrap();
        let plan = client.call("work.create", json!({"crewId":f.crew,"title":"Original plan","sources":[requirement["requirementId"]]})).await.unwrap();
        let foreign_crew = client
            .call(
                "crew.create",
                json!({"title":"Foreign work fixture","workingDirectory":f.data_dir()}),
            )
            .await
            .unwrap();
        let foreign = client.call("todo.create", json!({"crewId":foreign_crew["id"],"requestId":uuid::Uuid::new_v4().to_string(),"text":"Foreign request"})).await.unwrap();
        let requirement_before = client
            .call("todo.get", json!({"id":requirement["requirementId"]}))
            .await
            .unwrap();
        let plan_before = client
            .call("work.get", json!({"id":plan["id"]}))
            .await
            .unwrap();

        let workspace = client
            .call("data.workspace.get", json!({"crewId":f.crew}))
            .await
            .unwrap();
        let mut change = real02_document_change();
        change["edits"][1]["definition"]["placements"][0]["alias"] = json!("position");
        let saved = client.call("data.submit", json!({"context":workspace["context"],"requestId":uuid::Uuid::new_v4().to_string(),"change":change})).await.unwrap();
        let document = saved["created"]["dailyPage"]["object"]["reference"].clone();
        let body = saved["created"]["dailyKnowledge"]["object"]["reference"].clone();
        let base = saved["context"].clone();
        let work_ref =
            |kind: &str, id: &Value| json!({"namespace":base["namespace"],"kind":kind,"id":id});
        let link_edit = |reference: &Value| {
            json!({"kind":"createRelation","alias":"documentWorkLink",
            "subject":{"kind":"existing","reference":document},"predicate":compile::DOCUMENT_WORK_PREDICATE,
            "object":{"kind":"existing","reference":reference},"attributes":{}})
        };
        let req_ref = work_ref("requirement", &requirement["requirementId"]);
        let plan_ref = work_ref("plan", &plan["id"]);
        let foreign_ref = work_ref("requirement", &foreign["requirementId"]);
        let refused = request(&base, link_edit(&foreign_ref));
        assert_eq!(
            client
                .call("data.submit", refused.clone())
                .await
                .unwrap_err(),
            (
                RpcError::CONFLICT,
                "document work link rejected before commit".into()
            )
        );
        assert_eq!(
            client
                .call(
                    "data.request.get",
                    json!({"context":base,"requestId":refused["requestId"]})
                )
                .await
                .unwrap()["lookup"]["status"],
            json!("notFound"),
        );
        let mut wrong_namespace = plan_ref.clone();
        wrong_namespace["namespace"] = json!("other-namespace");
        assert!(
            client
                .call("data.submit", request(&base, link_edit(&wrong_namespace)))
                .await
                .is_err()
        );
        let wrong_kind = work_ref("plan", &requirement["requirementId"]);
        assert!(
            client
                .call("data.submit", request(&base, link_edit(&wrong_kind)))
                .await
                .is_err()
        );
        assert!(
            client
                .call("data.submit", request(&base, link_edit(&body)))
                .await
                .is_err(),
            "the reserved predicate cannot create a generic knowledge relation"
        );
        let web_bypass = json!({"kind":"createView","alias":"forbiddenWeb","title":"Forbidden Web","definition":{
            "kind":"web","renderer":"web","rendererVersion":"1","html":"","css":"","javascript":"",
            "bindings":[{"kind":"set","id":"work","source":{"kind":"object","reference":{"kind":"existing","reference":plan_ref}},"attributes":{}}],
            "requestedCapabilities":[],"attributes":{}},"sources":[],"attributes":{}});
        assert!(
            client
                .call("data.submit", request(&base, web_bypass))
                .await
                .is_err()
        );
        assert_eq!(
            client
                .call("data.workspace.get", json!({"crewId":f.crew}))
                .await
                .unwrap()["context"],
            base
        );

        let original = request(&base, link_edit(&req_ref));
        let req = client.call("data.submit", original.clone()).await.unwrap();
        let rel = req["created"]["documentWorkLink"]["object"]["reference"].clone();
        assert_eq!(
            client.call("data.submit", original.clone()).await.unwrap(),
            req
        );
        assert!(
            client
                .call("data.submit", request(&req["context"], link_edit(&req_ref)))
                .await
                .is_err()
        );
        let stale = client
            .call("data.submit", request(&base, link_edit(&plan_ref)))
            .await
            .unwrap_err();
        assert_eq!(stale.0, RpcError::CONFLICT);
        let plan_link = client
            .call(
                "data.submit",
                request(&req["context"], link_edit(&plan_ref)),
            )
            .await
            .unwrap();
        let current = plan_link["context"].clone();
        let list = client.call("data.object.list", json!({"context":current,"objectType":"relation","limit":50,"includeArchived":false})).await.unwrap();
        let items = list["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|r| r["object"]["reference"] == plan_ref));
        assert!(items.iter().any(|r| r["object"]["reference"] == req_ref));
        let detail = client
            .call(
                "data.object.get",
                json!({"context":current,"reference":rel}),
            )
            .await
            .unwrap();
        assert_eq!(detail["object"]["subject"]["reference"], document);
        assert_eq!(detail["object"]["object"]["reference"], req_ref);
        let work = detail["references"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["anchor"] == "relation:object")
            .unwrap();
        assert_eq!(work["complete"], true);
        assert_eq!(work["live"], true);
        let historical = client
            .call(
                "data.object.get",
                json!({"context":current,"reference":rel,"atRevision":"1"}),
            )
            .await
            .unwrap();
        assert_eq!(
            historical["object"]["object"],
            json!({"state":"unavailable"})
        );
        assert!(
            historical["references"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["anchor"] == "relation:object"
                    && r["complete"] == false
                    && r["live"] == false)
        );
        let history = client
            .call(
                "data.history",
                json!({"context":current,"reference":rel,"limit":50}),
            )
            .await
            .unwrap();
        assert_eq!(
            history["items"][0]["object"],
            json!({"state":"unavailable"})
        );

        let archived_doc = client.call("data.submit",request(&current,json!({"kind":"archiveView","id":document["id"],"expectedRevision":"1","archived":true}))).await.unwrap();
        let hidden = client
            .call(
                "data.object.get",
                json!({"context":archived_doc["context"],"reference":rel}),
            )
            .await
            .unwrap();
        assert_eq!(hidden["object"]["subject"], json!({"state":"unavailable"}));
        let archive_edit = |revision, archived| json!({"kind":"archiveRelation","id":rel["id"],"expectedRevision":revision,"archived":archived});
        let retired = client
            .call(
                "data.submit",
                request(&archived_doc["context"], archive_edit("1", true)),
            )
            .await
            .unwrap();
        assert!(
            client
                .call(
                    "data.submit",
                    request(&retired["context"], archive_edit("2", false))
                )
                .await
                .is_err()
        );
        let restored_doc = client.call("data.submit",request(&retired["context"],json!({"kind":"archiveView","id":document["id"],"expectedRevision":"2","archived":false}))).await.unwrap();
        let restored = client
            .call(
                "data.submit",
                request(&restored_doc["context"], archive_edit("2", false)),
            )
            .await
            .unwrap();
        assert_eq!(
            restored["changed"],
            json!([{"reference":rel,"revision":"3"}])
        );
        let latest = restored["context"].clone();
        assert!(client.call("data.submit",json!({"context":latest,"requestId":uuid::Uuid::new_v4().to_string(),
            "change":{"kind":"restoreObject","reference":rel,"revision":"1","expectedRevision":"3"}})).await.is_err());
        let mut old_epoch = latest.clone();
        old_epoch["restoreEpoch"] = json!("foreign-epoch");
        assert!(
            client
                .call("data.submit", request(&old_epoch, archive_edit("3", true)))
                .await
                .is_err()
        );
        assert_eq!(
            client
                .call("todo.get", json!({"id":requirement["requirementId"]}))
                .await
                .unwrap(),
            requirement_before
        );
        assert_eq!(
            client
                .call("work.get", json!({"id":plan["id"]}))
                .await
                .unwrap(),
            plan_before
        );

        let reopened = Arc::new(Runtime::new(rt.root.clone()).unwrap());
        let stored = reopened
            .handle(
                f.state.clone(),
                human(),
                Request {
                    id: 1,
                    method: "data.object.get".into(),
                    params: json!({"context":latest,"reference":rel}),
                },
            )
            .await
            .into_result()
            .unwrap();
        assert_eq!(stored["object"]["object"]["reference"], req_ref);
        assert_eq!(stored["object"]["subject"]["reference"], document);
        assert_eq!(stored["object"]["archived"], false);
        assert_eq!(
            client
                .call(
                    "data.object.get",
                    json!({"context":latest,"reference":body})
                )
                .await
                .unwrap()["object"]["content"],
            "日用版知识验收完成"
        );
        let known = client
            .call(
                "data.request.get",
                json!({"context":base,"requestId":original["requestId"]}),
            )
            .await
            .unwrap();
        assert_eq!(known["lookup"]["result"], req);
        let closed = ConnectionScope::new();
        closed.close();
        assert!(
            reopened
                .session(
                    f.state.clone(),
                    f.worker.clone(),
                    f.token(&f.worker),
                    closed,
                    "data.object.get",
                    json!({"context":latest,"reference":rel})
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn sqlite_real02_document_submit_exposes_placement_identity_contract() {
        let f = Fixture::new().await;
        let rt = runtime(&f).await;
        let scope = ConnectionScope::new();
        let workspace = rt
            .session(
                f.state.clone(),
                f.worker.clone(),
                f.token(&f.worker),
                scope.clone(),
                "data.workspace.get",
                json!({"crewId":f.crew}),
            )
            .await
            .unwrap();
        assert_eq!(workspace["state"], "uninitialized");
        let args = json!({"context":workspace["context"],"requestId":uuid::Uuid::new_v4().to_string(),"change":real02_document_change()});
        let error = rt
            .session(
                f.state.clone(),
                f.worker.clone(),
                f.token(&f.worker),
                scope,
                "data.submit",
                args,
            )
            .await
            .unwrap_err();
        println!(
            "real02 shape, fixture-owned authenticated worker route: {}",
            serde_json::to_string(&error).unwrap()
        );
        assert_eq!(error.code, RpcError::INVALID_PARAMS);
        assert_eq!(
            error.data.as_ref().unwrap().refusal,
            "data.placement.identity_required"
        );

        let f = Fixture::new().await;
        let _rt = runtime(&f).await;
        let socket = f.spawn_socket().await;
        let mut client = crate::agent::testkit::RpcClient::connect(&socket).await;
        client
            .call(
                methods::ATTACH,
                json!({"sessionId":f.worker,"token":f.token(&f.worker)}),
            )
            .await
            .unwrap();
        let workspace = client
            .call(
                methods::CALL,
                json!({"tool":"data_workspace_get","args":{"crewId":f.crew}}),
            )
            .await
            .unwrap();
        let workspace: Value = serde_json::from_str(workspace["text"].as_str().unwrap()).unwrap();
        let args = json!({"context":workspace["context"],"requestId":uuid::Uuid::new_v4().to_string(),"change":real02_document_change()});
        let result = client
            .call(methods::CALL, json!({"tool":"data_submit","args":args}))
            .await
            .unwrap();
        println!("real02 shape through production MCP: {result}");
        assert_eq!(result["isError"], true);
        assert!(
            result["text"]
                .as_str()
                .unwrap()
                .contains("data.placement.identity_required"),
            "a valid tool call must expose the safe input refusal, not hide it behind an unrelated ACL hint"
        );
    }

    #[tokio::test]
    async fn sqlite_real02_document_alias_commit_atomicity_and_reopen() {
        async fn call(
            client: &mut crate::agent::testkit::RpcClient,
            tool: &str,
            args: Value,
        ) -> Value {
            let result = client
                .call(methods::CALL, json!({"tool":tool,"args":args}))
                .await
                .unwrap();
            assert_eq!(result["isError"], false, "{result}");
            serde_json::from_str(result["text"].as_str().unwrap()).unwrap()
        }
        let f = Fixture::new().await;
        let rt = runtime(&f).await;
        let socket = f.spawn_socket().await;
        let mut client = crate::agent::testkit::RpcClient::connect(&socket).await;
        client
            .call(
                methods::ATTACH,
                json!({"sessionId":f.worker,"token":f.token(&f.worker)}),
            )
            .await
            .unwrap();
        let tools = client.call(methods::TOOLS, json!({})).await.unwrap();
        if let Some(path) = std::env::var_os("CREWD_TEST_KNOWLEDGE_TOOLS_OUTPUT") {
            std::fs::write(path, serde_json::to_vec_pretty(&tools).unwrap()).unwrap();
        }
        if let Some(path) = std::env::var_os("CREWD_TEST_KNOWLEDGE_SCHEMA_OUTPUT") {
            std::fs::write(
                path,
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&crewd_proto::api_schema()).unwrap()
                ),
            )
            .unwrap();
        }
        let workspace = call(&mut client, "data_workspace_get", json!({"crewId":f.crew})).await;
        let original = workspace["context"].clone();
        let refused=client.call(methods::CALL,json!({"tool":"data_submit","args":{"context":original,"requestId":uuid::Uuid::new_v4().to_string(),"change":real02_document_change()}})).await.unwrap();
        assert_eq!(refused["isError"], true);
        assert_eq!(
            call(&mut client, "data_workspace_get", json!({"crewId":f.crew})).await["state"],
            "uninitialized"
        );
        assert_eq!(
            std::fs::read_dir(&rt.root).unwrap().count(),
            0,
            "input-only rejection must not initialize or partially write knowledge"
        );

        // Same two edits; supply the explicitly required new placement identity.
        let mut change = real02_document_change();
        change["edits"][1]["definition"]["placements"][0]["alias"] = json!("dailyPlacement");
        let request = uuid::Uuid::new_v4().to_string();
        let args = json!({"context":original,"requestId":request,"change":change});
        let committed = call(&mut client, "data_submit", args.clone()).await;
        assert_eq!(committed["phase"], "committed");
        assert_eq!(committed["context"]["generation"], "1");
        assert_eq!(committed["created"].as_object().unwrap().len(), 3);
        let context = committed["context"].clone();
        let block = committed["created"]["dailyKnowledge"]["object"]["reference"].clone();
        let view = committed["created"]["dailyPage"]["object"]["reference"].clone();
        let placement = committed["created"]["dailyPlacement"]["id"].clone();
        let detail = call(
            &mut client,
            "data_object_get",
            json!({"context":context,"reference":view,"atRevision":null}),
        )
        .await;
        assert_eq!(
            detail["object"]["definition"]["placements"][0]["id"],
            placement
        );
        assert_eq!(
            detail["object"]["definition"]["placements"][0]["block"]["reference"],
            block
        );
        assert_eq!(
            call(
                &mut client,
                "data_request_get",
                json!({"context":original,"requestId":request})
            )
            .await["lookup"]["status"],
            "known"
        );
        let duplicate = call(&mut client, "data_submit", args).await;
        assert_eq!(
            duplicate["context"]["generation"], "1",
            "known original UUID is attribution, not a second commit"
        );

        // Early structure and later compiler/CAS failure both leave earlier
        // edits in the batch unapplied. Neither is classified as successful.
        let mut ambiguous = real02_document_change();
        ambiguous["edits"][1]["definition"]["placements"][0]["alias"] = json!("newPlacement");
        ambiguous["edits"][1]["definition"]["placements"][0]["id"] = placement.clone();
        let refused=client.call(methods::CALL,json!({"tool":"data_submit","args":{"context":context,"requestId":uuid::Uuid::new_v4().to_string(),"change":ambiguous}})).await.unwrap();
        assert!(
            refused["text"]
                .as_str()
                .unwrap()
                .contains("data.placement.identity_ambiguous")
        );
        let mixed = json!({"kind":"apply","message":"must roll back all edits","edits":[
            {"kind":"createBlock","alias":"mustNotExist","contentType":"text/plain","content":"not committed","sources":[],"attributes":{}},
            {"kind":"updateBlock","id":block["id"],"expectedRevision":"999","contentType":"text/plain","content":"wrong CAS","sources":null,"attributes":{}}
        ]});
        let refused=client.call(methods::CALL,json!({"tool":"data_submit","args":{"context":context,"requestId":uuid::Uuid::new_v4().to_string(),"change":mixed}})).await.unwrap();
        assert_eq!(refused["isError"], true);
        assert!(
            !refused["text"].as_str().unwrap().contains("wrong CAS"),
            "unrelated internal errors retain the conservative MCP message"
        );
        let page=call(&mut client,"data_object_list",json!({"context":context,"objectType":"block","after":null,"limit":100,"atRevision":null,"includeArchived":false})).await;
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(
            call(&mut client, "data_workspace_get", json!({"crewId":f.crew})).await["context"],
            context
        );

        let updated=call(&mut client,"data_submit",json!({"context":context,"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"apply","message":"update exact placement","edits":[
            {"kind":"updateView","id":view["id"],"expectedRevision":"1","title":"日用版知识验收","definition":{"kind":"document","layout":{},"placements":[{"kind":"set","id":placement,"alias":null,"block":{"kind":"existing","reference":block},"atRevision":null,"layout":{"updated":true}}]},"sources":null,"attributes":{}}
        ]}})).await;
        assert_eq!(updated["phase"], "committed");
        let detail = call(
            &mut client,
            "data_object_get",
            json!({"context":updated["context"],"reference":view,"atRevision":null}),
        )
        .await;
        assert_eq!(
            detail["object"]["definition"]["placements"][0]["id"],
            placement
        );
        let kept=call(&mut client,"data_submit",json!({"context":updated["context"],"requestId":uuid::Uuid::new_v4().to_string(),"change":{"kind":"apply","message":"keep exact placement","edits":[
            {"kind":"updateView","id":view["id"],"expectedRevision":"2","title":"日用版知识验收","definition":{"kind":"document","layout":{},"placements":[{"kind":"keep","id":placement,"layout":{"kept":true}}]},"sources":null,"attributes":{}}
        ]}})).await;
        assert_eq!(kept["phase"], "committed");
        // Each call already closes its SQLite connection. Reopen independently
        // and recheck the durable content plus stable placement association.
        let reopened = Arc::new(Runtime::new(rt.root.clone()).unwrap());
        let detail = reopened
            .session(
                f.state.clone(),
                f.worker.clone(),
                f.token(&f.worker),
                ConnectionScope::new(),
                "data.object.get",
                json!({"context":kept["context"],"reference":view,"atRevision":null}),
            )
            .await
            .unwrap();
        assert_eq!(
            detail["object"]["definition"]["placements"][0]["id"],
            placement
        );
        let content = reopened
            .session(
                f.state.clone(),
                f.worker.clone(),
                f.token(&f.worker),
                ConnectionScope::new(),
                "data.object.get",
                json!({"context":kept["context"],"reference":block,"atRevision":null}),
            )
            .await
            .unwrap();
        assert_eq!(content["object"]["content"], "日用版知识验收完成");
        let schema = f
            .state
            .db
            .call_result(|store| {
                store
                    .conn()
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            schema,
            crewd_store::schema::SCHEMA_VERSION,
            "current production schema retains resource17 without activating experimental Native16"
        );
        assert!(
            f.state
                .db
                .call_result(|store| crewd_store::meta_get(store.conn(), "teamData.format.v1"))
                .await
                .unwrap()
                .unwrap()
                .is_none(),
            "resource17 must not activate Native16 metadata"
        );
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
            {"kind":"createBlock","alias":"json","contentType":"application/json","content":{"flags":[true,null,3,{"nested":"value"}]},"sources":[],"attributes":{"preview":false}},
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
        let json_detail = agent(
            &rt,
            &f,
            "data.object.get",
            json!({
                "context":latest,
                "reference":result["created"]["json"]["object"]["reference"],
                "atRevision":null
            }),
        )
        .await;
        assert_eq!(
            json_detail["object"]["content"],
            json!({"flags":[true,null,3,{"nested":"value"}]})
        );
        assert_eq!(
            json_detail["object"]["provenance"]["attributes"],
            json!({"preview":false})
        );
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
        let output = std::env::var_os("CREWD_TEST_SQLITE_WIRE_OUTPUT")
            .map(PathBuf::from)
            .unwrap_or_else(|| f.data_dir().join("serde-wire.json"));
        std::fs::write(output, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
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
