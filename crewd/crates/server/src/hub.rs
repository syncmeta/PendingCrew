//! 进程内事件总线：客户端事件（`Event`，连接层按主题转发）与内部协作事件（`CollabEvent`，M3c / M4b 订阅）。
use crewd_collab::{Change, CollabError};
use crewd_proto::{ChannelRef, CrewRef, Event, events};
use rusqlite::Connection;
use serde::Serialize;
use tokio::sync::broadcast;

/// spec §22.5 的 `CollabEvent`：类型定义在 crewd-collab（裁定 3），这里是它在服务层的名字。
pub use crewd_collab::Change as CollabEvent;

#[cfg(any(test, feature = "testkit"))]
#[derive(Clone)]
pub struct SubscriptionCreations(std::sync::Arc<std::sync::atomic::AtomicU64>);
#[cfg(any(test, feature = "testkit"))]
impl SubscriptionCreations {
    pub fn count(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[derive(Clone)]
pub struct Hub {
    tx: broadcast::Sender<Event>,
    collab: broadcast::Sender<CollabEvent>,
    #[cfg(any(test, feature = "testkit"))]
    subscription_creations: SubscriptionCreations,
}

impl Hub {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(1024).0,
            collab: broadcast::channel(4096).0,
            #[cfg(any(test, feature = "testkit"))]
            subscription_creations: SubscriptionCreations(Default::default()),
        }
    }
    pub fn publish(&self, e: Event) {
        // 没有订阅者时 send 会报错，这正常，忽略。
        let _ = self.tx.send(e);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        let receiver = self.tx.subscribe();
        #[cfg(any(test, feature = "testkit"))]
        self.subscription_creations
            .0
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        receiver
    }
    #[cfg(any(test, feature = "testkit"))]
    pub fn subscription_creations(&self) -> SubscriptionCreations {
        self.subscription_creations.clone()
    }
    pub fn subscribe_collab(&self) -> broadcast::Receiver<CollabEvent> {
        self.collab.subscribe()
    }

    /// 提交之后、在库线程上调（`AppState::collab` 保证）：每条变更原样发内部通道，再现读数据发客户端事件。
    /// 取数失败只记日志（数据已经提交，不能让发事件的失败把成功的操作说成失败）。
    pub fn emit(&self, conn: &Connection, changes: &[Change]) {
        for c in changes {
            let _ = self.collab.send(c.clone());
            match client_events(conn, c) {
                Ok(evs) => evs.into_iter().for_each(|e| self.publish(e)),
                Err(e) => eprintln!("crewd: 变更 {c:?} 取数失败，客户端事件没发：{e}"),
            }
        }
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

fn ev<T: Serialize>(name: &str, data: &T) -> crewd_collab::Result<Event> {
    Ok(Event {
        event: name.into(),
        data: serde_json::to_value(data)?,
    })
}

/// 删掉了的东西读不到 = 不发（永久删除另有 `crew.purged` / `channel.purged`）。
fn gone<T>(r: crewd_collab::Result<T>) -> crewd_collab::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(CollabError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

fn client_events(conn: &Connection, c: &Change) -> crewd_collab::Result<Vec<Event>> {
    use crewd_collab::{asks, channels, crews, cursors, messages, restart};
    Ok(match c {
        Change::MessageAppended { message_id, .. } => gone(messages::get(conn, message_id))?
            .map(|m| ev(events::MESSAGE_APPENDED, &m))
            .transpose()?
            .into_iter()
            .collect(),
        Change::BadgeChanged { channel_id } => gone(cursors::badge(conn, channel_id))?
            .map(|b| ev(events::BADGE_CHANGED, &b))
            .transpose()?
            .into_iter()
            .collect(),
        Change::ChannelChanged { channel_id } => gone(channels::summary(conn, channel_id))?
            .map(|s| ev(events::CHANNEL_CHANGED, &s))
            .transpose()?
            .into_iter()
            .collect(),
        Change::CrewChanged { crew_id } => gone(crews::get(conn, crew_id))?
            .map(|x| ev(events::CREW_CHANGED, &x))
            .transpose()?
            .into_iter()
            .collect(),
        Change::CrewRemoved { crew_id } => vec![ev(
            events::CREW_PURGED,
            &CrewRef {
                crew_id: crew_id.clone(),
            },
        )?],
        Change::ChannelRemoved { channel_id } => vec![ev(
            events::CHANNEL_PURGED,
            &ChannelRef {
                channel_id: channel_id.clone(),
            },
        )?],
        Change::AskChanged { crew_id, ask } => gone(asks::get(conn, crew_id, ask))?
            .map(|a| ev(events::ASK_CHANGED, &a))
            .transpose()?
            .into_iter()
            .collect(),
        Change::RestartChanged { id } => gone(restart::get(conn, *id))?
            .map(|p| ev(events::RESTART_CHANGED, &p))
            .transpose()?
            .into_iter()
            .collect(),
        // 只走内部通道：session.changed（SessionLive）由 M3c 发；入队 / 答复 / 重启答复是给 M3c / M4b 的。
        Change::SessionChanged { .. }
        | Change::WakeEnqueued { .. }
        | Change::AskAnswered { .. }
        | Change::RestartAnswered { .. }
        | Change::Progress { .. } => vec![],
    })
}
