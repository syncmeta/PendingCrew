use crewd_proto::Event;
use tokio::sync::broadcast;

/// 进程内事件总线。订阅者处理太慢会收到 Lagged，由连接层转成 `events.lagged`。
#[derive(Clone)]
pub struct Hub {
    tx: broadcast::Sender<Event>,
}

impl Hub {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(1024).0,
        }
    }
    pub fn publish(&self, e: Event) {
        // 没有订阅者时 send 会报错，这正常，忽略。
        let _ = self.tx.send(e);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}
