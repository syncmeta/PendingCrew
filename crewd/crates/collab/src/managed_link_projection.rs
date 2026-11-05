//! One explicit checked WorkBook per read/patch, shared by all target projections.
use super::*;
pub(crate) struct Projection<'a> {
    pub c: &'a Connection,
    pub actor: Actor<'a>,
    pub external: &'a dyn ExternalTargets,
    book: Option<crewd_work::WorkBook>,
    pub book_loads: usize,
}
impl<'a> Projection<'a> {
    pub fn new(c: &'a Connection, actor: Actor<'a>, external: &'a dyn ExternalTargets) -> Self {
        Self {
            c,
            actor,
            external,
            book: None,
            book_loads: 0,
        }
    }
    fn book(&mut self) -> Result<&crewd_work::WorkBook> {
        if self.book.is_none() {
            self.book = Some(crate::asks::load_org(self.c)?);
            self.book_loads += 1;
        }
        Ok(self.book.as_ref().expect("initialized checked work book"))
    }
    fn work(&mut self, id: &str) -> Result<(object_refs::ObjectReference, crewd_work::WorkItem)> {
        let r = reference(self.c, id, 'W', ObjectKind::Plan)?;
        let local = crate::work::core_plan(self.c, &r)?;
        let item =
            self.book()?.item(local).cloned().ok_or_else(|| {
                StoreError::Invalid("work target has no authoritative item".into())
            })?;
        Ok((r, item))
    }
    fn ask(&mut self, id: &str) -> Result<(object_refs::ObjectReference, crewd_work::HumanAsk)> {
        let r = reference(self.c, id, 'H', ObjectKind::HumanTask)?;
        resource_acl::actor(self.c, &sender(self.actor), &r.crew_id, false)?;
        let core = crewd_store::work_organization::core_for_reference(
            self.c,
            ObjectKind::HumanTask,
            r.number,
        )?
        .and_then(|s| s.parse::<crewd_work::AskId>().ok())
        .ok_or_else(|| StoreError::Invalid("Ask target lacks authoritative identity".into()))?;
        let h =
            self.book()?.ask(core).cloned().ok_or_else(|| {
                StoreError::Invalid("Ask target missing authoritative source".into())
            })?;
        Ok((r, h))
    }
    pub fn resolve(&mut self, t: &TargetRef, mode: ResolveMode) -> Result<ResolvedTarget> {
        self.actor.principal(self.c)?;
        let (summary, crew) = match t {
            TargetRef::Work { id } => {
                let (r, w) = self.work(id)?;
                (w.title, r.crew_id)
            }
            TargetRef::Requirement { id } => {
                let r = reference(self.c, id, '#', ObjectKind::Requirement)?;
                let c = self.c;
                let d = crate::requirements::detail(c, &r, self.book()?)?;
                (d.todo.text, r.crew_id)
            }
            TargetRef::Ask { id } => {
                let (r, h) = self.ask(id)?;
                (h.question, r.crew_id)
            }
            _ => return resolve_uncached(self.c, self.actor, t, self.external, mode),
        };
        Ok(ResolvedTarget {
            target: t.clone(),
            summary: summary.chars().take(280).collect(),
            owner_crew_id: Some(crew),
            message_channel_id: None,
        })
    }
    pub fn source(&mut self, s: &SourceRef, write: bool) -> Result<String> {
        let resolved = self.resolve(&source_target(s), ResolveMode::Read)?;
        let crew = match s {
            SourceRef::Artifact {
                metadata_crew_id, ..
            } => {
                crews::get(self.c, metadata_crew_id)?;
                metadata_crew_id.clone()
            }
            _ => resolved.owner_crew_id.ok_or_else(forbidden)?,
        };
        if !write {
            return Ok(crew);
        }
        let c = self.c;
        let a = self.actor;
        match s {
            SourceRef::Artifact { .. } => {
                a.owner(c, &crew, true)?;
            }
            SourceRef::Row { id } => {
                let (t, _, owner, _) =
                    tables::row_base(c, num(id, 'O')?)?.ok_or_else(|| missing(id))?;
                crate::managed_tables_v2::row_authority(c, a, &t, &owner)?;
            }
            SourceRef::Table { id } => {
                let t = tables::table(c, num(id, 'T')?)?.ok_or_else(|| missing(id))?;
                crate::managed_tables_v2::schema_authority(c, a, &t)?;
            }
            SourceRef::Decision { id } | SourceRef::Risk { id } | SourceRef::Lesson { id } => {
                let sub = match s {
                    SourceRef::Decision { .. } => Subject::Decision { id: id.clone() },
                    SourceRef::Risk { .. } => Subject::Risk { id: id.clone() },
                    _ => Subject::Lesson { id: id.clone() },
                };
                objects::artifact_link_authority(c, a, &sub)?;
            }
            SourceRef::Work { id } => {
                resource_acl::actor(c, &sender(a), &crew, true)?;
                if let Actor::Session(session) = a {
                    let (_, w) = self.work(id)?;
                    if w.owner.as_ref().map(|v| v.as_str()) != Some(session)
                        && !crews::acts_as_captain_of(c, session, &crew)?
                    {
                        return Err(forbidden());
                    }
                }
            }
            SourceRef::Requirement { .. } => {
                resource_acl::actor(c, &sender(a), &crew, true)?;
                if let Actor::Session(session) = a
                    && !crews::acts_as_captain_of(c, session, &crew)?
                {
                    return Err(forbidden());
                }
            }
            SourceRef::Ask { id } => {
                resource_acl::actor(c, &sender(a), &crew, true)?;
                if let Actor::Session(session) = a {
                    let (_, h) = self.ask(id)?;
                    let own = matches!(h.asked_by,crewd_work::Actor::Session{ref id,..}|crewd_work::Actor::ScopedSession{ref id,..} if id.as_str()==session);
                    if !own && !crews::acts_as_captain_of(c, session, &crew)? {
                        return Err(forbidden());
                    }
                }
            }
        }
        Ok(crew)
    }
    pub fn declare(&mut self, crew: &str, v: &ManualTargetWrite, now: &str) -> Result<TargetRef> {
        match v {
            ManualTargetWrite::Reference { target } => {
                self.resolve(target, ResolveMode::FreshWrite)?;
                Ok(target.clone())
            }
            ManualTargetWrite::DeclareArtifact { input } => {
                let r = artifacts::canonicalize(crew, input).map_err(|_| invalid("artifact"))?;
                artifacts::ensure(self.c, &r, now)?;
                Ok(TargetRef::Artifact { reference: r })
            }
        }
    }
    pub fn edge(&mut self, e: &db::Edge) -> Result<EdgeView> {
        self.source(&e.source, false)?;
        let target = self.resolve(&e.target, ResolveMode::Read)?;
        let origin = if e.origin == "manual" {
            EdgeOrigin::ManualRelates {}
        } else {
            EdgeOrigin::Column {
                field_id: e
                    .origin
                    .strip_prefix("column:")
                    .ok_or_else(|| invalid("origin"))?
                    .into(),
            }
        };
        let editable = e.origin == "manual" && self.source(&e.source, true).is_ok();
        Ok(EdgeView {
            id: e.id.to_string(),
            owner_crew_id: e.owner.clone(),
            source: e.source.clone(),
            target: target.target,
            summary: target.summary,
            origin,
            editable,
        })
    }
    fn business_refs(&mut self, s: &SourceRef) -> Result<(String, Vec<(String, TargetRef)>)> {
        let owner = self.source(s, false)?;
        let c = self.c;
        let mut refs: Vec<(String, TargetRef)> = Vec::new();
        match s {
            SourceRef::Work { id } => {
                let (_, w) = self.work(id)?;
                let plans = |id| crate::work::reference(c, ObjectKind::Plan, &owner, id);
                if let Some(id) = w.parent {
                    refs.push(("work.parent".into(), TargetRef::Work { id: plans(id)? }))
                }
                for id in w.depends_on {
                    refs.push(("work.dependsOn".into(), TargetRef::Work { id: plans(id)? }))
                }
                for id in w.sources {
                    refs.push((
                        "work.sources".into(),
                        TargetRef::Requirement {
                            id: crate::work::reference(c, ObjectKind::Requirement, &owner, id)?,
                        },
                    ))
                }
                for b in w.blocked_on {
                    match b {
                        crewd_work::Blocker::Human(id) => refs.push((
                            "work.blockedOn".into(),
                            TargetRef::Ask {
                                id: crate::work::reference(c, ObjectKind::HumanTask, &owner, id)?,
                            },
                        )),
                        crewd_work::Blocker::Item(id) => {
                            refs.push(("work.blockedOn".into(), TargetRef::Work { id: plans(id)? }))
                        }
                    }
                }
                if let Some(id) = w.owner {
                    refs.push((
                        "work.owner".into(),
                        TargetRef::Session { id: id.to_string() },
                    ))
                }
            }
            SourceRef::Decision { id } | SourceRef::Risk { id } | SourceRef::Lesson { id } => {
                let o = objects::get_with_defaults(c, self.actor, id, true)?;
                match &o {
                    Object::Decision { decision } => {
                        if let Some(id) = &decision.supersedes {
                            refs.push((
                                "decision.supersedes".into(),
                                TargetRef::Decision { id: id.clone() },
                            ));
                        }
                        if let Some(id) = &decision.superseded_by {
                            refs.push((
                                "decision.supersededBy".into(),
                                TargetRef::Decision { id: id.clone() },
                            ));
                        }
                        if let Some(origin) = crate::decision_asks::origin(c, &decision.id)? {
                            refs.push((
                                "decision.origin".into(),
                                TargetRef::Ask { id: origin.ask },
                            ));
                        }
                    }
                    Object::Risk { risk } => {
                        if let Some(id) = &risk.became_lesson_id {
                            refs.push((
                                "risk.becameLesson".into(),
                                TargetRef::Lesson { id: id.clone() },
                            ));
                        }
                    }
                    Object::Lesson { lesson } => {
                        if let Some(id) = &lesson.risk_id {
                            refs.push(("lesson.risk".into(), TargetRef::Risk { id: id.clone() }));
                        }
                    }
                    _ => (),
                }
                let value = serde_json::to_value(o)?;
                for object in ["decision", "risk", "lesson"] {
                    if let Some(v) = value.get(object).and_then(|v| v.get("draft")) {
                        for f in ["about", "mitigatedBy", "learnedFrom", "appliesTo"] {
                            if let Some(array) = v.get(f).and_then(|v| v.as_array()) {
                                for v in array {
                                    if let Ok(t) = serde_json::from_value::<TargetRef>(v.clone()) {
                                        refs.push((format!("{object}.{f}"), t))
                                    }
                                }
                            }
                        }
                    }
                }
            }
            SourceRef::Ask { id } => {
                let (_, h) = self.ask(id)?;
                if let Some(id) = h.item {
                    refs.push((
                        "ask.item".into(),
                        TargetRef::Work {
                            id: crate::work::reference(c, ObjectKind::Plan, &owner, id)?,
                        },
                    ))
                }
            }
            _ => (),
        }
        if let Some(n) = db::source_number(s)?
            && let Some(snapshot) = crewd_store::managed_message_sources::get(c, n)?
            && let Some(id) = snapshot.message_id
        {
            refs.push(("source.message".into(), TargetRef::Message { id }));
        }
        refs.sort_by(|(an, at), (bn, bt)| {
            adapter_id(s, an, at)
                .unwrap_or_default()
                .cmp(&adapter_id(s, bn, bt).unwrap_or_default())
        });
        refs.dedup_by(|(an, at), (bn, bt)| an == bn && at == bt);
        Ok((owner, refs))
    }
    fn project_adapter(
        &mut self,
        s: &SourceRef,
        owner: &str,
        name: String,
        t: TargetRef,
    ) -> Result<Option<EdgeView>> {
        match self.resolve(&t, ResolveMode::Read) {
            Ok(v) => {
                let id = adapter_id(s, &name, &t)?;
                Ok(Some(EdgeView {
                    id: id.clone(),
                    owner_crew_id: owner.into(),
                    source: s.clone(),
                    target: t,
                    summary: v.summary,
                    origin: EdgeOrigin::Adapter {
                        adapter: name,
                        readonly_edge_id: id,
                    },
                    editable: false,
                }))
            }
            Err(e) if hidden_error(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }
    /// Real raw rows consumed, including duplicates/hidden targets, count toward
    /// the same request budget. No full legacy artifact child table is loaded.
    pub fn adapter_chunk(
        &mut self,
        s: &SourceRef,
        position: &AdapterPosition,
        budget: usize,
    ) -> Result<AdapterChunk> {
        if budget == 0 || position.stage > 2 {
            return Err(invalid("adapterCursor"));
        }
        let (owner, refs) = self.business_refs(s)?;
        let mut items = Vec::new();
        let mut raw_rows = 0;
        let mut pos = position.clone();
        if pos.stage == 0 {
            for (name, t) in refs {
                let id = adapter_id(s, &name, &t)?;
                if id <= pos.after {
                    continue;
                }
                if raw_rows == budget {
                    return Ok(AdapterChunk {
                        items,
                        next: Some(pos),
                        raw_rows,
                    });
                }
                raw_rows += 1;
                pos.after = id;
                if let Some(v) = self.project_adapter(s, &owner, name, t)? {
                    items.push((pos.clone(), v));
                }
            }
            pos = AdapterPosition {
                stage: 1,
                after: "0".into(),
            };
        }
        if raw_rows == budget {
            return Ok(AdapterChunk {
                items,
                next: Some(pos),
                raw_rows,
            });
        }
        if pos.stage == 1 {
            let n = db::source_number(s)?;
            let after = pos
                .after
                .parse::<i64>()
                .ok()
                .filter(|n| *n >= 0 && n.to_string() == pos.after)
                .ok_or_else(changed)?;
            if let Some(n) = n {
                let limit = (budget - raw_rows) as u16;
                let rows = db::artifact_adapter_rows(self.c, n, after, limit)?;
                let full = rows.len() == usize::from(limit);
                for (id, reference, relation, first) in rows {
                    raw_rows += 1;
                    pos.after = id.to_string();
                    if !first {
                        continue;
                    }
                    if let Some(v) = self.project_adapter(
                        s,
                        &owner,
                        format!("artifact.{relation}"),
                        TargetRef::Artifact { reference },
                    )? {
                        items.push((pos.clone(), v));
                    }
                }
                if full {
                    return Ok(AdapterChunk {
                        items,
                        next: Some(pos),
                        raw_rows,
                    });
                }
            }
        }
        Ok(AdapterChunk {
            items,
            next: None,
            raw_rows,
        })
    }
}
