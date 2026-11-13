//! First-use publication facts. No filesystem, model, message or wake operations.
//!
//! Content belongs exclusively to the ordinary per-crew Knowledge database.
//! This bounded metadata contains original identities/digests and publication
//! phases, never document text or an execution/permission certificate. The
//! server holds the data-directory lock and keeps admission closed throughout.
use crate::{Change, CollabError, Result, Tx};
use crewd_proto::{ChannelKnowledgeReference, Crew, ResourceActor, team_data::*};
use crewd_store::{StoreError, meta_get, meta_set};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const KEY: &str = "onboarding.core.v1";
pub const FRESH_KEY: &str = "onboarding.fresh.v1";
pub const ROOT: &str = "pendingcrew-welcome";
pub const PRINCIPAL: &str = "builtin:onboarding.core.v1";
pub const CREWS: &[(&str, &str, &[&str])] = &[
    (ROOT, "PendingCrew\u{600e}\u{4e48}\u{7528}", &[]),
    ("pendingcrew-welcome-demo", "\u{6f14}\u{793a}", &[ROOT]),
    ("pendingcrew-welcome-questions", "\u{95ee}\u{7b54}", &[ROOT]),
    (
        "pendingcrew-welcome-child",
        "\u{5b50}\u{673a}\u{7ec4}",
        &["pendingcrew-welcome-demo", "pendingcrew-welcome-questions"],
    ),
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Asset {
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    pub asset: Asset,
    pub view: String,
    pub block: String,
    pub placement: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Publication {
    pub edition: u32,
    pub request: String,
    pub origin: DataContext,
    pub payload_sha256: String,
    pub created_at: String,
    pub pages: Vec<Page>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct State {
    pub format: u32,
    pub db_id: String,
    /// skipped is a durable decision, even if the old organization is empty.
    pub skipped: bool,
    pub ready: Vec<Publication>,
    pub pending: Option<Publication>,
}
fn invalid(s: &str) -> CollabError {
    StoreError::Invalid(s.into()).into()
}
fn uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|u| u.to_string() == s)
}
fn sha(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_asset(a: &Asset) -> bool {
    !a.file.is_empty()
        && a.file.len() <= 96
        && a.file.ends_with(".md")
        && a.file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        && !a.file.starts_with('.')
        && a.bytes <= 1024 * 1024
        && sha(&a.sha256)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootLifecycle {
    Missing,
    Active,
    Removed,
}
/// Fixed-ID original-row read; never list the whole organization for a derived
/// projection. This is a view of existing crew authority, not another ledger.
pub fn root_lifecycle(c: &Connection) -> Result<RootLifecycle> {
    let row: Option<Option<String>> = c
        .query_row("SELECT archived_at FROM crews WHERE id=?1", [ROOT], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(match row {
        None => RootLifecycle::Missing,
        Some(None) => RootLifecycle::Active,
        Some(Some(_)) => RootLifecycle::Removed,
    })
}
fn identity(c: &Connection) -> Result<String> {
    meta_get(c, "dbId")?.ok_or_else(|| invalid("onboarding database identity missing"))
}
fn validate(s: &State, db: &str) -> Result<()> {
    if s.format != 1
        || s.db_id != db
        || (db.is_empty() || db.len() > 4096)
        || s.ready.len() > 32
        || (s.skipped && (!s.ready.is_empty() || s.pending.is_some()))
    {
        return Err(invalid(
            "onboarding publication state unknown; original retained",
        ));
    }
    let mut files = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut last = 0;
    for p in s.ready.iter().chain(s.pending.iter()) {
        if p.edition <= last
            || p.pages.is_empty()
            || p.pages.len() > 64
            || !uuid(&p.request)
            || !sha(&p.payload_sha256)
            || p.created_at.is_empty()
            || p.origin.db_id != db
            || p.origin.authority_id != db
            || p.origin.crew_id != ROOT
            || p.origin.namespace != crewd_store::team_data::namespace(db, ROOT)
            || p.origin.restore_epoch.is_empty()
            || p.origin.generation.parse::<u64>().is_err()
        {
            return Err(invalid("onboarding original publication identity invalid"));
        }
        last = p.edition;
        if !ids.insert(&p.request) {
            return Err(invalid("duplicate onboarding request identity"));
        }
        for page in &p.pages {
            if !valid_asset(&page.asset) || !files.insert(&page.asset.file) {
                return Err(invalid("onboarding version filename or digest conflict"));
            }
            for id in [&page.view, &page.block, &page.placement] {
                if !uuid(id) || !ids.insert(id) {
                    return Err(invalid("onboarding stable identity conflict"));
                }
            }
        }
        let digest = crewd_store::team_data::digest(&(p.edition, &p.pages))?;
        if digest != p.payload_sha256 {
            return Err(invalid("onboarding original plan digest differs"));
        }
    }
    Ok(())
}
pub fn read(c: &Connection) -> Result<Option<State>> {
    let Some(raw) = meta_get(c, KEY)? else {
        return Ok(None);
    };
    if raw.len() > 256 * 1024 {
        return Err(invalid("onboarding state bound exceeded"));
    }
    let s: State = serde_json::from_str(&raw).map_err(StoreError::from)?;
    validate(&s, &identity(c)?)?;
    Ok(Some(s))
}
fn encoded(c: &Connection, s: &State) -> Result<String> {
    validate(s, &identity(c)?)?;
    let raw = serde_json::to_string(s).map_err(StoreError::from)?;
    if raw.len() > 256 * 1024 {
        return Err(invalid("onboarding state bound exceeded"));
    }
    Ok(raw)
}
fn write(tx: &mut Tx<'_>, s: &State) -> Result<()> {
    let raw = encoded(tx.conn(), s)?;
    meta_set(tx.conn(), KEY, &raw)?;
    Ok(())
}
/// Establish fresh/skipped exactly once. Fresh origin is supplied only by the
/// Store initialization transaction, not inferred from an empty crew listing.
pub fn begin(tx: &mut Tx<'_>) -> Result<State> {
    if let Some(s) = read(tx.conn())? {
        return Ok(s);
    }
    let db = identity(tx.conn())?;
    let fresh = meta_get(tx.conn(), FRESH_KEY)?.as_deref() == Some("1");
    let occupied: i64 = tx
        .conn()
        .query_row("SELECT count(*) FROM crews", [], |r| r.get(0))?;
    let s = State {
        format: 1,
        db_id: db,
        skipped: !fresh || occupied != 0,
        ready: vec![],
        pending: None,
    };
    write(tx, &s)?;
    Ok(s)
}
/// Only new version filenames may be issued. The original body is never an
/// update payload, and a ready identity is never a resurrection obligation.
pub fn prepare(
    tx: &mut Tx<'_>,
    edition: u32,
    assets: &[Asset],
    origin: DataContext,
) -> Result<State> {
    let mut s = begin(tx)?;
    if s.skipped {
        return Ok(s);
    }
    if assets.is_empty() || assets.len() > 64 || assets.iter().any(|a| !valid_asset(a)) {
        return Err(invalid("onboarding asset bounds invalid"));
    }
    let mut names = BTreeSet::new();
    if assets.iter().any(|a| !names.insert(&a.file)) {
        return Err(invalid("duplicate onboarding asset name"));
    }
    for old in s.ready.iter().flat_map(|p| &p.pages) {
        if let Some(asset) = assets.iter().find(|a| a.file == old.asset.file)
            && *asset != old.asset
        {
            return Err(invalid(
                "published onboarding filename changed; add a new version",
            ));
        }
    }
    if let Some(p) = &s.pending {
        // Resume uses the persisted original context/IDs, never a fresh plan.
        for page in &p.pages {
            if !assets.iter().any(|a| a == &page.asset) {
                return Err(invalid("pending onboarding bundle unavailable or changed"));
            }
        }
        return Ok(s);
    }
    let pages: Vec<Page> = assets
        .iter()
        .filter(|a| {
            !s.ready
                .iter()
                .flat_map(|p| &p.pages)
                .any(|old| old.asset.file == a.file)
        })
        .map(|asset| Page {
            asset: asset.clone(),
            view: uuid::Uuid::new_v4().to_string(),
            block: uuid::Uuid::new_v4().to_string(),
            placement: uuid::Uuid::new_v4().to_string(),
        })
        .collect();
    if pages.is_empty() {
        return Ok(s);
    }
    if s.ready.last().is_some_and(|p| edition <= p.edition) {
        return Err(invalid("onboarding edition must advance"));
    }
    if !s.ready.is_empty() {
        // A missing or removed tutorial root is a human lifecycle choice.
        if root_lifecycle(tx.conn())? != RootLifecycle::Active {
            return Ok(s);
        }
    }
    let payload_sha256 = crewd_store::team_data::digest(&(edition, &pages))?;
    s.pending = Some(Publication {
        edition,
        request: uuid::Uuid::new_v4().to_string(),
        origin,
        payload_sha256,
        created_at: tx.now_ts(),
        pages,
    });
    // Prove both A and its eventual C representation fit BEFORE B can commit.
    // Reaching the edition/serialized bound cannot leave a permanently
    // unpublishable pending request in the other database.
    let mut committed = s.clone();
    committed.ready.push(
        committed
            .pending
            .take()
            .ok_or_else(|| invalid("onboarding plan missing"))?,
    );
    encoded(tx.conn(), &committed)?;
    write(tx, &s)?;
    Ok(s)
}
fn expected_versions(p: &Publication) -> Vec<DataObjectVersion> {
    p.pages
        .iter()
        .flat_map(|page| [(&page.block, "block"), (&page.view, "view")])
        .map(|(id, kind)| DataObjectVersion {
            reference: DataRef {
                namespace: p.origin.namespace.clone(),
                kind: kind.into(),
                id: id.clone(),
            },
            revision: "1".into(),
        })
        .collect()
}
/// Server supplies only the original Knowledge receipt obtained at the locked
/// startup boundary. This is not public input and does not grant actor authority.
pub fn finish(tx: &mut Tx<'_>, receipt: &DataChangeResult) -> Result<State> {
    let mut s = read(tx.conn())?.ok_or_else(|| invalid("onboarding plan missing"))?;
    let p = s
        .pending
        .clone()
        .ok_or_else(|| invalid("onboarding has no pending publication"))?;
    let mut expected_context = p.origin.clone();
    let generation = p
        .origin
        .generation
        .parse::<u64>()
        .map_err(|_| invalid("onboarding generation invalid"))?
        .checked_add(1)
        .ok_or_else(|| invalid("onboarding generation exhausted"))?;
    expected_context.generation = generation.to_string();
    if receipt.request_id != p.request
        || receipt.context != expected_context
        || !matches!(receipt.phase, DataPhase::Committed)
        || receipt.before_generation != p.origin.generation
        || receipt.after_generation != expected_context.generation
        || serde_json::to_value(&receipt.changed).map_err(StoreError::from)?
            != serde_json::to_value(expected_versions(&p)).map_err(StoreError::from)?
        || !receipt.created.is_empty()
        || !receipt.conflicts.is_empty()
        || receipt.reason.is_some()
        || receipt.snapshot_id.is_some()
        || receipt.resolved_by_request.is_some()
    {
        return Err(invalid("onboarding original Knowledge receipt disagrees"));
    }
    if s.ready.is_empty() {
        let occupied: i64 = tx
            .conn()
            .query_row("SELECT count(*) FROM crews", [], |r| r.get(0))?;
        if occupied != 0 {
            return Err(invalid(
                "fresh onboarding organization changed before publication",
            ));
        }
        let stored = meta_get(tx.conn(), "nextCrewNumber")?;
        let mut number = match stored {
            Some(n) => n
                .parse::<i64>()
                .map_err(|_| invalid("onboarding crew allocator invalid"))?
                .max(1),
            None => 1,
        };
        for (id, title, _) in CREWS {
            let crew = Crew {
                id: (*id).into(),
                title: (*title).into(),
                number: Some(number),
                title_source: Some("auto".into()),
                created_at: p.created_at.clone(),
                updated_at: p.created_at.clone(),
                ..Default::default()
            };
            number = number
                .checked_add(1)
                .ok_or_else(|| invalid("onboarding crew allocator exhausted"))?;
            crewd_store::crews::upsert(tx.conn(), &crew, None)?;
            crewd_store::channels::ensure_lobby(tx.conn(), id, &p.created_at)?;
            tx.note(Change::CrewChanged {
                crew_id: (*id).into(),
            });
            tx.note(Change::ChannelChanged {
                channel_id: crewd_store::channels::lobby_id(id),
            });
        }
        meta_set(tx.conn(), "nextCrewNumber", &number.to_string())?;
        for (id, _, parents) in CREWS {
            for parent in *parents {
                crate::crews::adopt(tx, id, parent)?;
            }
        }
        let m = crewd_store::crews::ensure_management(tx.conn(), &p.created_at)?;
        tx.note(Change::CrewChanged { crew_id: m.clone() });
        tx.note(Change::ChannelChanged {
            channel_id: crewd_store::channels::lobby_id(&m),
        });
    }
    if root_lifecycle(tx.conn())? != RootLifecycle::Active {
        return Err(invalid(
            "onboarding root removed during pending publication",
        ));
    }
    let channel = crewd_store::channels::lobby_id(ROOT);
    let actor = ResourceActor {
        kind: "human".into(),
        session_id: None,
    };
    let mut revision = crewd_store::channel_work::revision(tx.conn(), &channel)?;
    for page in &p.pages {
        let source = ChannelKnowledgeReference {
            context: receipt.context.clone(),
            reference: DataRef {
                namespace: p.origin.namespace.clone(),
                kind: "view".into(),
                id: page.view.clone(),
            },
        };
        revision = crewd_store::knowledge_channels::change(
            tx.conn(),
            &channel,
            &source,
            revision,
            true,
            &actor,
            &p.created_at,
        )?;
    }
    tx.note(Change::ChannelWorkChanged {
        channel_id: channel,
        link_revision: revision.to_string(),
    });
    s.ready.push(p);
    s.pending = None;
    write(tx, &s)?;
    Ok(s)
}
