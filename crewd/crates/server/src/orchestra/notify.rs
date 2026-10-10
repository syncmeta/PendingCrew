//! Transition-only home-human notices. Memory-only: after restart a still-broken state raises again.
//! Give-ups wait one fixed ten-minute window so its one line contains all session counts.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crewd_collab::render::safe_name;
use crewd_wake::text::{self, Lang};
use crewd_wake::{MIN, Ms};

use super::port::{Audience, Port};

pub const WINDOW_MS: i64 = 10 * MIN;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    LaunchFailed,
    Quota,
    BriefUndelivered,
    BinaryReplaced,
    InputDraft,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub kind: Kind,
    pub scope: String,
}
impl Key {
    pub fn new(kind: Kind, scope: &str) -> Self {
        Self {
            kind,
            scope: scope.into(),
        }
    }
}
/// Successful real notifier posts, observed only by the synthetic Core harness.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct Transition {
    pub key: Key,
    pub raise: bool,
    pub crew: String,
    pub text: String,
}
struct GiveUps {
    due: Ms,
    sessions: BTreeMap<String, (String, usize)>,
    error: String,
}
pub struct Notifier<P: Port> {
    port: Arc<P>,
    raised: HashMap<Key, String>,
    give_ups: HashMap<String, GiveUps>,
    #[cfg(test)]
    transitions: Vec<Transition>,
}
impl<P: Port> Notifier<P> {
    pub fn new(port: Arc<P>) -> Self {
        Self {
            port,
            raised: HashMap::new(),
            give_ups: HashMap::new(),
            #[cfg(test)]
            transitions: Vec::new(),
        }
    }
    #[cfg(test)]
    pub(crate) fn transitions(&self) -> &[Transition] {
        &self.transitions
    }

    pub fn is_raised(&self, key: &Key) -> bool {
        self.raised.contains_key(key)
    }
    pub async fn raise(&mut self, key: Key, crew: &str, text: String) -> anyhow::Result<()> {
        if self.raised.contains_key(&key) {
            return Ok(());
        }
        #[cfg(test)]
        let observed_text = text.clone();
        self.port
            .post_system(
                Audience::HumanChannel {
                    crew: crew.into(),
                    mention_human: true,
                },
                text,
            )
            .await?;
        #[cfg(test)]
        self.transitions.push(Transition {
            key: key.clone(),
            raise: true,
            crew: crew.into(),
            text: observed_text,
        });
        self.raised.insert(key, crew.into());
        Ok(())
    }
    pub async fn clear(&mut self, key: &Key, text: String) -> anyhow::Result<()> {
        let Some(crew) = self.raised.get(key) else {
            return Ok(());
        };
        #[cfg(test)]
        let observed_text = text.clone();
        self.port
            .post_system(
                Audience::HumanChannel {
                    crew: crew.clone(),
                    mention_human: false,
                },
                text,
            )
            .await?;
        #[cfg(test)]
        self.transitions.push(Transition {
            key: key.clone(),
            raise: false,
            crew: crew.clone(),
            text: observed_text,
        });
        self.raised.remove(key);
        Ok(())
    }
    pub fn give_up(
        &mut self,
        crew: &str,
        session: &str,
        name: &str,
        count: usize,
        error: &str,
        now: Ms,
    ) {
        let window = self.give_ups.entry(crew.into()).or_insert_with(|| GiveUps {
            due: now.add(WINDOW_MS),
            sessions: BTreeMap::new(),
            error: String::new(),
        });
        let entry = window
            .sessions
            .entry(session.into())
            .or_insert_with(|| (safe_name(name), 0));
        entry.1 += count;
        window.error = safe_name(error);
    }
    pub fn next_deadline(&self) -> Option<Ms> {
        self.give_ups.values().map(|w| w.due).min()
    }
    pub async fn flush(&mut self, now: Ms, lang: Lang) -> anyhow::Result<()> {
        let due: HashSet<String> = self
            .give_ups
            .iter()
            .filter(|(_, w)| w.due <= now)
            .map(|(c, _)| c.clone())
            .collect();
        for crew in due {
            let w = &self.give_ups[&crew];
            let sessions = w
                .sessions
                .values()
                .map(|(name, n)| format!("{name} ({n})"))
                .collect::<Vec<_>>()
                .join(", ");
            let text = text::render(
                lang,
                "notice.give_up.crew",
                &[("sessions", &sessions), ("error", &w.error)],
            );
            self.port
                .post_system(
                    Audience::HumanChannel {
                        crew: crew.clone(),
                        mention_human: true,
                    },
                    text,
                )
                .await?;
            self.give_ups.remove(&crew);
        }
        Ok(())
    }
}
