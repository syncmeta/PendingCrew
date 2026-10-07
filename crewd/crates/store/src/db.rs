//! 专用线程上的数据库 actor（spec §22.2；§14 / §18 遗留：RPC 不在 tokio 线程上同步调 SQLite）。
//! 一条名为 `crewd-db` 的线程独占 `Store`；`call` 按提交顺序串行执行闭包。
use crate::{Result, Store, StoreError};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

type Job = Box<dyn FnOnce(&mut Store) + Send>;

enum Msg {
    Job(Job),
    /// 在它之前入队的任务跑完后线程退出（接收端随之关闭，之后的 `call` 得 `DbClosed`）。
    Stop,
}

/// 可克隆的句柄。所有句柄都 drop 之后线程退出、库关闭；也可显式 `shutdown`。
#[derive(Clone)]
pub struct Db {
    tx: mpsc::Sender<Msg>,
    /// 线程句柄由所有克隆共享；`shutdown` 取走它来 join。
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl Db {
    /// 把 `store` 交给库线程；之后只能经 `call` 用它。
    pub fn spawn(store: Store) -> std::io::Result<Db> {
        let (tx, rx) = mpsc::channel::<Msg>();
        let handle = std::thread::Builder::new()
            .name("crewd-db".into())
            .spawn(move || {
                let mut store = store;
                while let Ok(Msg::Job(job)) = rx.recv() {
                    job(&mut store);
                }
                // 收尾（S16）：optimize + 把 WAL 折回主库并截断，之后连接随 store 一起关。
                if let Err(e) = store.finish() {
                    eprintln!("crewd: 关库收尾没做完：{e}");
                }
            })?;
        Ok(Db {
            tx,
            thread: Arc::new(Mutex::new(Some(handle))),
        })
    }

    /// 关库：已入队的任务先跑完，然后线程退出并被 join（库连接随之关闭）。其它克隆此后的 `call` 得
    /// `DbClosed`。**阻塞**调用线程直到库线程退出：在 async 里用 `spawn_blocking` 包一层；
    /// 不要在库线程自己的任务里调（会等自己）。重复调用（任一克隆）无害。
    pub fn shutdown(self) {
        let _ = self.tx.send(Msg::Stop);
        let handle = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    /// 在库线程上跑 `f`（与其它调用串行）。`call` 是惰性的：任务在返回的 future
    /// **第一次被轮询**时才入队，所以执行顺序 = 各 future 第一次被轮询的顺序。
    pub async fn call<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Store) -> T + Send + 'static,
    {
        let (done, wait) = tokio::sync::oneshot::channel::<Result<T>>();
        let job: Job = Box::new(move |s| {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(s)));
            let _ = done.send(r.map_err(|_| StoreError::DbJobPanicked));
        });
        self.tx
            .send(Msg::Job(job))
            .map_err(|_| StoreError::DbClosed)?;
        wait.await.map_err(|_| StoreError::DbClosed)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::Poll;
    use std::time::{Duration, Instant};

    fn db() -> Db {
        Db::spawn(Store::open_in_memory().unwrap()).unwrap()
    }

    /// 按顺序反复轮询一组 future 直到全部完成（不依赖 futures crate）。
    /// 第一轮按下标顺序轮询：`call` 在第一次被轮询时才把任务送进队列，
    /// 所以任务的提交顺序 = 下标顺序，且全部提交完成之前没有一个会先被等到。
    async fn join_in_order<T>(mut futs: Vec<Pin<Box<dyn Future<Output = T> + '_>>>) -> Vec<T> {
        let mut out: Vec<Option<T>> = futs.iter().map(|_| None).collect();
        std::future::poll_fn(|cx| {
            let mut pending = false;
            for (i, f) in futs.iter_mut().enumerate() {
                if out[i].is_some() {
                    continue;
                }
                match f.as_mut().poll(cx) {
                    Poll::Ready(v) => out[i] = Some(v),
                    Poll::Pending => pending = true,
                }
            }
            if pending {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
        out.into_iter().map(|v| v.unwrap()).collect()
    }

    #[tokio::test]
    async fn shutdown_lets_queued_jobs_finish_then_joins_the_thread() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let db = db();
        let clone = db.clone();
        let (a, b) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let (a2, b2) = (a.clone(), b.clone());
        // 直接入队两个任务（第一个慢），再 shutdown：返回时两个都必须已跑完。
        db.tx
            .send(Msg::Job(Box::new(move |_| {
                std::thread::sleep(Duration::from_millis(150));
                a2.store(true, Ordering::SeqCst);
            })))
            .unwrap();
        db.tx
            .send(Msg::Job(Box::new(move |_| {
                b2.store(true, Ordering::SeqCst)
            })))
            .unwrap();
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
        assert!(a.load(Ordering::SeqCst) && b.load(Ordering::SeqCst));
        // 其它克隆此后调用 = 库已关。
        let r: Result<()> = clone.call(|_| ()).await;
        assert!(matches!(r, Err(StoreError::DbClosed)), "{r:?}");
        // 再 shutdown 一次（克隆上）也不挂、不 panic。
        tokio::task::spawn_blocking(move || clone.shutdown())
            .await
            .unwrap();
    }

    /// S16：关库前把 WAL 折回主库并截成 0（另有一条连接开着时，SQLite 关连接不会自己清 WAL）。
    #[tokio::test]
    async fn shutdown_checkpoints_and_truncates_the_wal() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("crewd.db");
        let db = Db::spawn(Store::open(&p).unwrap()).unwrap();
        // 另一条连接（比如备份 / 外部只读工具）开着：最后一个关的不是库线程。
        let other = rusqlite::Connection::open(&p).unwrap();
        db.call(|s| {
            s.conn()
                .execute_batch("CREATE TABLE t(v TEXT); INSERT INTO t VALUES('示例');")
                .unwrap()
        })
        .await
        .unwrap();
        // 让它真的连上（打开是惰性的）。
        let _: i64 = other
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        let wal = d.path().join("crewd.db-wal");
        assert!(std::fs::metadata(&wal).unwrap().len() > 0);
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
        assert_eq!(std::fs::metadata(&wal).unwrap().len(), 0, "WAL 已截断");
        let n: i64 = other
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_slow_job_does_not_block_the_runtime() {
        let db = db();
        let slow = {
            let db = db.clone();
            tokio::spawn(async move {
                db.call(|_| std::thread::sleep(Duration::from_millis(400)))
                    .await
            })
        };
        // 计时必须在 yield 之前开始：若任务被内联跑在 runtime 线程上，卡顿就发生在 yield 里。
        let t = Instant::now();
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            t.elapsed() < Duration::from_millis(200),
            "单线程 runtime 被库调用卡住了：{:?}",
            t.elapsed()
        );
        slow.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn jobs_run_in_submission_order() {
        let db = db();
        db.call(|s| s.conn().execute_batch("CREATE TABLE t(v INTEGER)").unwrap())
            .await
            .unwrap();
        // 50 个调用先按顺序全部创建、并发提交，再一起等；执行顺序必须等于提交顺序。
        let db_ref = &db;
        let futs: Vec<Pin<Box<dyn Future<Output = Result<()>> + '_>>> = (0..50i64)
            .map(|i| {
                Box::pin(db_ref.call(move |s| {
                    s.conn()
                        .execute("INSERT INTO t(v) VALUES(?1)", [i])
                        .unwrap();
                })) as Pin<Box<dyn Future<Output = Result<()>> + '_>>
            })
            .collect();
        for r in join_in_order(futs).await {
            r.unwrap();
        }
        let got: Vec<i64> = db
            .call(|s| {
                let mut st = s.conn().prepare("SELECT v FROM t ORDER BY rowid").unwrap();
                st.query_map([], |r| r.get(0))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(got, (0..50).collect::<Vec<i64>>());
    }

    #[tokio::test]
    async fn a_panicking_job_is_reported_and_the_thread_survives() {
        let db = db();
        let r: Result<()> = db.call(|_| panic!("boom")).await;
        assert!(matches!(r, Err(StoreError::DbJobPanicked)), "{r:?}");
        let n: i64 = db
            .call(|s| {
                s.conn()
                    .query_row("SELECT 41 + 1", [], |r| r.get(0))
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(n, 42);
    }

    #[tokio::test]
    async fn a_panic_inside_a_transaction_rolls_it_back() {
        let db = db();
        db.call(|s| s.conn().execute_batch("CREATE TABLE t(v INTEGER)").unwrap())
            .await
            .unwrap();
        let _ = db
            .call(|s| {
                let _ = s.with_tx(|tx| -> Result<()> {
                    tx.execute("INSERT INTO t(v) VALUES(1)", [])?;
                    panic!("half way");
                });
            })
            .await;
        let n: i64 = db
            .call(|s| {
                s.conn()
                    .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
