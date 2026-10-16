//! Converts authorized public edits into a normalized SQLite transaction batch. The caller
//! holds the deterministic namespace lock cut and rechecks it before dispatch.
use crewd_proto::{ResourceActor, team_data::*};
use crewd_store::team_data::{Allocation, PlacementParent};
use crewd_store::team_data::{Capture, Mutation};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone)]
pub struct Stored {
    pub object: DataObject,
    pub captures: Vec<Capture>,
}
pub struct Target {
    pub commit: Option<String>,
    pub revision: Option<String>,
}
pub trait Port {
    fn object(&self, reference: &DataRef, at: Option<&str>) -> Result<Stored, String>;
    /// Must verify actual current actor/target ACL and exact requested snapshot.
    fn target(&self, reference: &DataRef, at: Option<&str>) -> Result<Target, String>;
}
fn reference(namespace: &str, kind: &str, id: &str) -> DataRef {
    DataRef {
        namespace: namespace.into(),
        kind: kind.into(),
        id: id.into(),
    }
}
fn next(value: &str) -> Result<String, String> {
    let old = value
        .parse::<u64>()
        .map_err(|_| "invalid object revision")?;
    if old == 0 || old.to_string() != value {
        return Err("invalid canonical revision".into());
    }
    old.checked_add(1)
        .map(|v| v.to_string())
        .ok_or("revision exhausted".into())
}
fn fields(o: &DataObject) -> (&str, &str, &str, &DataProvenance) {
    match o {
        DataObject::Block {
            id,
            revision,
            provenance,
            ..
        } => ("block", id, revision, provenance),
        DataObject::View {
            id,
            revision,
            provenance,
            ..
        } => ("view", id, revision, provenance),
        DataObject::Relation {
            id,
            revision,
            provenance,
            ..
        } => ("relation", id, revision, provenance),
    }
}
pub fn allocations(context: &DataContext, edits: &[DataEdit]) -> Result<Vec<Allocation>, String> {
    if edits.is_empty() || edits.len() > 64 {
        return Err("invalid batch count".into());
    }
    let mut out = vec![];
    for e in edits {
        let created = match e {
            DataEdit::CreateBlock { alias, .. } | DataEdit::ForkBlock { alias, .. } => {
                Some((alias, "block"))
            }
            DataEdit::CreateView { alias, .. } => Some((alias, "view")),
            DataEdit::CreateRelation { alias, .. } => Some((alias, "relation")),
            _ => None,
        };
        if let Some((alias, kind)) = created {
            out.push(Allocation::Object {
                alias: alias.clone(),
                kind: kind.into(),
            })
        }
        let view = match e {
            DataEdit::CreateView {
                alias, definition, ..
            } => Some((PlacementParent::Created(alias.clone()), definition)),
            DataEdit::UpdateView {
                id,
                expected_revision,
                definition,
                ..
            } => Some((
                PlacementParent::Existing(DataObjectVersion {
                    reference: reference(&context.namespace, "view", id),
                    revision: next(expected_revision)?,
                }),
                definition,
            )),
            _ => None,
        };
        if let Some((parent, DataViewDefinitionInput::Document { placements, .. })) = view {
            for p in placements {
                if let DataPlacementEdit::Set {
                    id: None,
                    alias: Some(alias),
                    ..
                } = p
                {
                    out.push(Allocation::Placement {
                        alias: alias.clone(),
                        parent: parent.clone(),
                    })
                }
            }
        }
    }
    if out.len() > 8192 {
        return Err("created identity bound exceeded".into());
    }
    Ok(out)
}
struct Compiler<'a> {
    context: &'a DataContext,
    created: &'a BTreeMap<String, DataCreated>,
    actor: &'a ResourceActor,
    now: &'a str,
    port: &'a dyn Port,
}
impl Compiler<'_> {
    fn created(&self, alias: &str, kind: Option<&str>) -> Result<DataObjectVersion, String> {
        match self.created.get(alias) {
            Some(DataCreated::Object { object })
                if kind.is_none_or(|k| k == object.reference.kind) =>
            {
                Ok(object.clone())
            }
            _ => Err("unknown creation alias or wrong kind".into()),
        }
    }
    fn resolve(
        &self,
        input: &DataInputRef,
        at: Option<&str>,
        anchor: &str,
        captures: &mut Vec<Capture>,
        expected: Option<&str>,
    ) -> Result<DataRef, String> {
        let (r, target) = match input {
            DataInputRef::Created { alias } => {
                if at.is_some() {
                    return Err("new alias cannot name a past commit".into());
                }
                let version = self.created(alias, expected)?;
                (
                    version.reference,
                    Target {
                        commit: Some("self".into()),
                        revision: Some(version.revision),
                    },
                )
            }
            DataInputRef::Existing { reference } => {
                if expected.is_some_and(|k| reference.kind != k) {
                    return Err("reference kind mismatch".into());
                }
                (reference.clone(), self.port.target(reference, at)?)
            }
        };
        if ["block", "view", "relation"].contains(&r.kind.as_str()) {
            let commit = if r.namespace == self.context.namespace && at.is_none() {
                "self".into()
            } else {
                target.commit.ok_or("target exact snapshot missing")?
            };
            captures.push(Capture {
                anchor: anchor.into(),
                namespace: r.namespace.clone(),
                kind: r.kind.clone(),
                id: r.id.clone(),
                commit,
                revision: target.revision.ok_or("target revision missing")?,
                live: at.is_none(),
            });
        } else if at.is_some() {
            return Err("Core link cannot use a historical object revision".into());
        }
        Ok(r)
    }
    fn sources(
        &self,
        inputs: &[DataSourceInput],
        captures: &mut Vec<Capture>,
    ) -> Result<Vec<DataSource>, String> {
        if inputs.len() > 4096 {
            return Err("source count bound exceeded".into());
        }
        inputs
            .iter()
            .enumerate()
            .map(|(n, s)| {
                Ok(DataSource {
                    reference: s
                        .reference
                        .as_ref()
                        .map(|r| {
                            self.resolve(
                                r,
                                s.at_revision.as_deref(),
                                &format!("source:{n}"),
                                captures,
                                None,
                            )
                            .map(|reference| DataRefState::Visible { reference })
                        })
                        .transpose()?,
                    at_revision: s.at_revision.clone(),
                    attributes: s.attributes.clone(),
                })
            })
            .collect()
    }
    fn old(&self, kind: &str, id: &str, expected: &str) -> Result<Stored, String> {
        let old = self
            .port
            .object(&reference(&self.context.namespace, kind, id), None)?;
        let (k, key, rev, _) = fields(&old.object);
        if k != kind || key != id || rev != expected {
            return Err("object revision conflict".into());
        }
        Ok(old)
    }
    fn provenance(
        &self,
        old: Option<&DataProvenance>,
        sources: Option<&[DataSourceInput]>,
        attrs: Option<&Value>,
        captures: &mut Vec<Capture>,
    ) -> Result<DataProvenance, String> {
        let mut p = old.cloned().unwrap_or_else(|| DataProvenance {
            created_by: self.actor.clone(),
            updated_by: self.actor.clone(),
            created_at: self.now.into(),
            updated_at: self.now.into(),
            sources: vec![],
            attributes: Value::Null,
        });
        p.updated_by = self.actor.clone();
        p.updated_at = self.now.into();
        if let Some(s) = sources {
            p.sources = self.sources(s, captures)?
        }
        if let Some(a) = attrs {
            p.attributes = a.clone()
        }
        Ok(p)
    }
    fn definition(
        &self,
        input: &DataViewDefinitionInput,
        old: Option<&DataViewDefinition>,
        parent: &DataObjectVersion,
        captures: &mut Vec<Capture>,
    ) -> Result<DataViewDefinition, String> {
        match input {
            DataViewDefinitionInput::Document { placements, layout } => {
                captures.retain(|c| !c.anchor.starts_with("binding:"));
                if placements.len() > 4096 {
                    return Err("placement count exceeded".into());
                }
                let previous = match old {
                    Some(DataViewDefinition::Document { placements, .. }) => placements.as_slice(),
                    _ => &[],
                };
                let mut ids = BTreeSet::new();
                let mut out = vec![];
                for p in placements {
                    let placement = match p {
                        DataPlacementEdit::Keep { id, layout } => {
                            let mut old = previous
                                .iter()
                                .find(|p| &p.id == id)
                                .ok_or("Keep requires same parent placement")?
                                .clone();
                            old.layout = layout.clone();
                            old
                        }
                        DataPlacementEdit::Set {
                            id,
                            alias,
                            block,
                            at_revision,
                            layout,
                        } => {
                            let id = match (id, alias) {
                                (Some(id), None) if previous.iter().any(|p| &p.id == id) => {
                                    id.clone()
                                }
                                (None, Some(alias)) => match self.created.get(alias) {
                                    Some(DataCreated::Placement { id, parent_view })
                                        if parent_view.reference == parent.reference =>
                                    {
                                        id.clone()
                                    }
                                    _ => return Err("placement alias parent mismatch".into()),
                                },
                                _ => {
                                    return Err(
                                        "placement Set needs exact old id or new alias".into()
                                    );
                                }
                            };
                            captures.retain(|c| c.anchor != format!("placement:{id}"));
                            let r = self.resolve(
                                block,
                                at_revision.as_deref(),
                                &format!("placement:{id}"),
                                captures,
                                Some("block"),
                            )?;
                            DataPlacement {
                                id,
                                block: DataRefState::Visible { reference: r },
                                at_revision: at_revision.clone(),
                                layout: layout.clone(),
                            }
                        }
                    };
                    if !ids.insert(placement.id.clone()) {
                        return Err("duplicate placement ID".into());
                    }
                    out.push(placement);
                }
                captures.retain(|c| {
                    !c.anchor.starts_with("placement:")
                        || ids.contains(c.anchor.strip_prefix("placement:").unwrap_or(""))
                });
                Ok(DataViewDefinition::Document {
                    placements: out,
                    layout: layout.clone(),
                })
            }
            DataViewDefinitionInput::Web {
                renderer,
                renderer_version,
                html,
                css,
                javascript,
                bindings,
                requested_capabilities,
                attributes,
            } => {
                captures.retain(|c| !c.anchor.starts_with("placement:"));
                if html.len() + css.len() + javascript.len() > 1024 * 1024 || bindings.len() > 4096
                {
                    return Err("web bundle bound exceeded".into());
                }
                let previous = match old {
                    Some(DataViewDefinition::Web { bindings, .. }) => bindings.as_slice(),
                    _ => &[],
                };
                let mut names = BTreeSet::new();
                let mut out = vec![];
                for b in bindings {
                    let value = match b {
                        DataBindingEdit::Keep { id } => previous
                            .iter()
                            .find(|b| &b.id == id)
                            .ok_or("Keep requires same parent binding")?
                            .clone(),
                        DataBindingEdit::Set {
                            id,
                            source,
                            attributes,
                        } => {
                            captures.retain(|c| c.anchor != format!("binding:{id}"));
                            let source = match source {
                                DataBindingInputSource::Query { statement } => {
                                    DataBindingSource::Query {
                                        statement: statement.clone(),
                                    }
                                }
                                DataBindingInputSource::Objects { object_type, limit } => {
                                    if !["block", "view", "relation"]
                                        .contains(&object_type.as_str())
                                        || *limit == 0
                                        || *limit > 100
                                    {
                                        return Err("invalid Objects binding".into());
                                    }
                                    DataBindingSource::Objects {
                                        object_type: object_type.clone(),
                                        limit: *limit,
                                    }
                                }
                                DataBindingInputSource::Object {
                                    reference,
                                    at_revision,
                                } => {
                                    let reference = self.resolve(
                                        reference,
                                        at_revision.as_deref(),
                                        &format!("binding:{id}"),
                                        captures,
                                        None,
                                    )?;
                                    if !["block", "view", "relation"]
                                        .contains(&reference.kind.as_str())
                                    {
                                        return Err(
                                            "Object binding requires a knowledge object".into()
                                        );
                                    }
                                    DataBindingSource::Object {
                                        reference: DataRefState::Visible { reference },
                                        at_revision: at_revision.clone(),
                                    }
                                }
                            };
                            DataBinding {
                                id: id.clone(),
                                source,
                                attributes: attributes.clone(),
                            }
                        }
                    };
                    if value.id.is_empty()
                        || value.id.len() > 128
                        || !names.insert(value.id.clone())
                    {
                        return Err("duplicate or invalid binding name".into());
                    }
                    out.push(value);
                }
                captures.retain(|c| {
                    !c.anchor.starts_with("binding:")
                        || names.contains(c.anchor.strip_prefix("binding:").unwrap_or(""))
                });
                Ok(DataViewDefinition::Web {
                    renderer: renderer.clone(),
                    renderer_version: renderer_version.clone(),
                    html: html.clone(),
                    css: css.clone(),
                    javascript: javascript.clone(),
                    bindings: out,
                    requested_capabilities: requested_capabilities.clone(),
                    attributes: attributes.clone(),
                })
            }
        }
    }
}
pub fn compile(
    context: &DataContext,
    created: &BTreeMap<String, DataCreated>,
    actor: &ResourceActor,
    now: &str,
    edits: &[DataEdit],
    port: &dyn Port,
) -> Result<Vec<Mutation>, String> {
    let compiler = Compiler {
        context,
        created,
        actor,
        now,
        port,
    };
    let mut out = vec![];
    let mut touched = BTreeSet::new();
    for edit in edits {
        let mut captures = vec![];
        let (object, expected) = match edit {
            DataEdit::CreateBlock {
                alias,
                content_type,
                content,
                sources,
                attributes,
            } => {
                let v = compiler.created(alias, Some("block"))?;
                let provenance =
                    compiler.provenance(None, Some(sources), Some(attributes), &mut captures)?;
                (
                    DataObject::Block {
                        id: v.reference.id,
                        revision: "1".into(),
                        content_type: content_type.clone(),
                        content: content.clone(),
                        archived: false,
                        provenance,
                    },
                    None,
                )
            }
            DataEdit::UpdateBlock {
                id,
                expected_revision,
                content_type,
                content,
                sources,
                attributes,
            } => {
                let old = compiler.old("block", id, expected_revision)?;
                captures = old.captures;
                if sources.is_some() {
                    captures.retain(|c| !c.anchor.starts_with("source:"));
                }
                let DataObject::Block {
                    archived,
                    provenance,
                    ..
                } = old.object
                else {
                    unreachable!()
                };
                let provenance = compiler.provenance(
                    Some(&provenance),
                    sources.as_deref(),
                    Some(attributes),
                    &mut captures,
                )?;
                (
                    DataObject::Block {
                        id: id.clone(),
                        revision: next(expected_revision)?,
                        content_type: content_type.clone(),
                        content: content.clone(),
                        archived,
                        provenance,
                    },
                    Some(expected_revision.clone()),
                )
            }
            DataEdit::ForkBlock {
                alias,
                source,
                at_revision,
                attributes,
            } => {
                let source_object = port.object(source, at_revision.as_deref())?;
                let DataObject::Block {
                    content_type,
                    content,
                    ..
                } = source_object.object
                else {
                    return Err("fork source must be block".into());
                };
                let v = compiler.created(alias, Some("block"))?;
                let sources = [DataSourceInput {
                    reference: Some(DataInputRef::Existing {
                        reference: source.clone(),
                    }),
                    at_revision: at_revision.clone(),
                    attributes: Value::Null,
                }];
                let provenance =
                    compiler.provenance(None, Some(&sources), Some(attributes), &mut captures)?;
                (
                    DataObject::Block {
                        id: v.reference.id,
                        revision: "1".into(),
                        content_type,
                        content,
                        archived: false,
                        provenance,
                    },
                    None,
                )
            }
            DataEdit::CreateView {
                alias,
                title,
                definition,
                sources,
                attributes,
            } => {
                let v = compiler.created(alias, Some("view"))?;
                let definition = compiler.definition(definition, None, &v, &mut captures)?;
                let provenance =
                    compiler.provenance(None, Some(sources), Some(attributes), &mut captures)?;
                (
                    DataObject::View {
                        id: v.reference.id,
                        revision: "1".into(),
                        title: title.clone(),
                        definition,
                        archived: false,
                        provenance,
                    },
                    None,
                )
            }
            DataEdit::UpdateView {
                id,
                expected_revision,
                title,
                definition,
                sources,
                attributes,
            } => {
                let old = compiler.old("view", id, expected_revision)?;
                captures = old.captures;
                if sources.is_some() {
                    captures.retain(|c| !c.anchor.starts_with("source:"));
                }
                let DataObject::View {
                    definition: previous,
                    provenance,
                    archived,
                    ..
                } = old.object
                else {
                    unreachable!()
                };
                let v = DataObjectVersion {
                    reference: reference(&context.namespace, "view", id),
                    revision: next(expected_revision)?,
                };
                let definition =
                    compiler.definition(definition, Some(&previous), &v, &mut captures)?;
                let provenance = compiler.provenance(
                    Some(&provenance),
                    sources.as_deref(),
                    Some(attributes),
                    &mut captures,
                )?;
                (
                    DataObject::View {
                        id: id.clone(),
                        revision: v.revision,
                        title: title.clone(),
                        definition,
                        archived,
                        provenance,
                    },
                    Some(expected_revision.clone()),
                )
            }
            DataEdit::CreateRelation {
                alias,
                subject,
                predicate,
                object,
                attributes,
            } => {
                let v = compiler.created(alias, Some("relation"))?;
                let subject =
                    compiler.resolve(subject, None, "relation:subject", &mut captures, None)?;
                let object =
                    compiler.resolve(object, None, "relation:object", &mut captures, None)?;
                let provenance = compiler.provenance(None, None, None, &mut captures)?;
                (
                    DataObject::Relation {
                        id: v.reference.id,
                        revision: "1".into(),
                        subject: DataRefState::Visible { reference: subject },
                        predicate: predicate.clone(),
                        object: DataRefState::Visible { reference: object },
                        attributes: attributes.clone(),
                        archived: false,
                        provenance,
                    },
                    None,
                )
            }
            DataEdit::ArchiveBlock {
                id,
                expected_revision,
                archived,
            }
            | DataEdit::ArchiveView {
                id,
                expected_revision,
                archived,
            }
            | DataEdit::ArchiveRelation {
                id,
                expected_revision,
                archived,
            } => {
                let kind = match edit {
                    DataEdit::ArchiveBlock { .. } => "block",
                    DataEdit::ArchiveView { .. } => "view",
                    _ => "relation",
                };
                let mut old = compiler.old(kind, id, expected_revision)?;
                captures = old.captures;
                match &mut old.object {
                    DataObject::Block {
                        revision,
                        archived: flag,
                        provenance,
                        ..
                    }
                    | DataObject::View {
                        revision,
                        archived: flag,
                        provenance,
                        ..
                    }
                    | DataObject::Relation {
                        revision,
                        archived: flag,
                        provenance,
                        ..
                    } => {
                        *revision = next(expected_revision)?;
                        *flag = *archived;
                        provenance.updated_by = actor.clone();
                        provenance.updated_at = now.into();
                    }
                }
                (old.object, Some(expected_revision.clone()))
            }
            DataEdit::Sql { statements } => {
                if statements.is_empty() || statements.len() > 64 {
                    return Err("invalid SQL statement batch".into());
                }
                for s in statements {
                    out.push(Mutation::Sql {
                        sql: s.sql.clone(),
                        params: s.params.clone(),
                    });
                }
                continue;
            }
        };
        let (kind, id, _, _) = fields(&object);
        if !touched.insert((kind.to_owned(), id.to_owned())) {
            return Err("one explicit edit per object per batch required".into());
        }
        out.push(Mutation::PutObject {
            object,
            expected_revision: expected,
            captures,
        });
    }
    if out.len() > 64 {
        return Err("normalized mutation count exceeded".into());
    }
    // A live local reference is captured at this publication, not at a stale
    // pre-batch revision. Forward references and an existing block edited later
    // in this same batch both resolve to the final normalized object revision.
    let staged: BTreeMap<_, _> = out
        .iter()
        .filter_map(|m| match m {
            Mutation::PutObject { object, .. } => {
                let (k, id, revision, _) = fields(object);
                Some(((k.to_owned(), id.to_owned()), revision.to_owned()))
            }
            _ => None,
        })
        .collect();
    for mutation in &mut out {
        if let Mutation::PutObject { captures, .. } = mutation {
            for capture in captures
                .iter_mut()
                .filter(|c| c.live && c.namespace == context.namespace && c.commit == "self")
            {
                if let Some(revision) = staged.get(&(capture.kind.clone(), capture.id.clone())) {
                    capture.revision = revision.clone();
                } else {
                    let target = port.target(
                        &reference(&capture.namespace, &capture.kind, &capture.id),
                        None,
                    )?;
                    capture.revision = target
                        .revision
                        .ok_or("local live reference revision unavailable")?;
                }
            }
        }
    }
    Ok(out)
}
