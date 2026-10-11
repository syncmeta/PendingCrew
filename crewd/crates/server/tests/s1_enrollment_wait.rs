//! Actual external kernel peer + true registry guard + production conn/writer/drain/Hub.
//! The library daemon and guard are real; this is not the driver-held D/R sibling fixture.
#![cfg(feature = "testkit")]
mod support {
    pub mod t11;
}
use crewd_exec::registry::{Enrollment, ProcessRegistry};
use crewd_server::hub::SubscriptionCreations;
use serde_json::{Value, json};
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    process::ExitStatus,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use support::t11::{CHILD_PATH, Fixture, artifact};
struct Catch<F: Future>(Pin<Box<F>>);
impl<F: Future> Future for Catch<F> {
    type Output = Result<F::Output, Box<dyn std::any::Any + Send>>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.get_mut().0.as_mut().poll(cx)
        })) {
            Ok(Poll::Ready(x)) => Poll::Ready(Ok(x)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(p) => Poll::Ready(Err(p)),
        }
    }
}
async fn wait_path(path: &Path) -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .map_err(io::Error::other)
}
fn pending_path(control: &Path, pid: u32, method: &str) -> PathBuf {
    let tag = method
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    control.join(format!("pending-{pid}-{tag}.json"))
}
fn read_json(path: &Path) -> io::Result<Value> {
    serde_json::from_slice(&std::fs::read(path)?).map_err(io::Error::other)
}
fn selected_rows(f: &Fixture) -> io::Result<(i64, i64, i64)> {
    let c = rusqlite::Connection::open_with_flags(
        f.data.join(crewd_store::DB_FILE),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(io::Error::other)?;
    let count = |title: &str| {
        c.query_row("SELECT count(*) FROM crews WHERE title=?1", [title], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(io::Error::other)
    };
    Ok((
        count("S1_STAGE3_POSITIVE")?,
        count("S1_STAGE3_DENIED")?,
        count("S1_STAGE3_AFTER_SETTLEMENT")?,
    ))
}
fn start_child(
    f: &Fixture,
    tag: &str,
    method: &str,
    params: Value,
    half_close: bool,
    slot: &mut Option<tokio::process::Child>,
) -> io::Result<PathBuf> {
    let dir = f.root.path().join(tag);
    std::fs::create_dir(&dir)?;
    std::fs::write(
        dir.join("request.json"),
        serde_json::to_vec(&json!({"socket":f.data.join("crewd.sock"),
        "request":{"id":933,"method":method,"params":params},"halfCloseAfterResponse":half_close}))
        .map_err(io::Error::other)?,
    )?;
    let log = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join("stderr.log"))?;
    let child = tokio::process::Command::new(artifact("fake_claude"))
        .args(["--s1-stage3-wire-client"])
        .arg(&dir)
        .env_clear()
        .env("PATH", CHILD_PATH)
        .env("HOME", &f.home)
        .env("SHELL", "/usr/bin/false")
        .env("XDG_CACHE_HOME", &f.cache)
        .env("CLAUDE_CONFIG_DIR", &f.config)
        .env("TMPDIR", f.root.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log)
        .kill_on_drop(true)
        .spawn()?;
    *slot = Some(child);
    Ok(dir)
}
async fn finish_child(
    slot: &mut Option<tokio::process::Child>,
    dir: &Path,
) -> io::Result<(ExitStatus, Value)> {
    let child = slot
        .as_mut()
        .ok_or_else(|| io::Error::other("missing owned child"))?;
    let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
        .await
        .map_err(io::Error::other)??;
    let receipt = read_json(&dir.join("wire.receipt.json"))?;
    slot.take();
    Ok((status, receipt))
}
fn response(receipt: &Value) -> &Value {
    let frames = receipt["frames"]
        .as_array()
        .expect("actual raw wire frames");
    let matches = frames.iter().filter(|v| v["id"] == 933).collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "one exact original request response, no replay"
    );
    matches[0]
}
async fn run_case(drain_case: bool) {
    let f = Fixture::new();
    let control = f.root.path().join("control");
    std::fs::create_dir(&control).unwrap();
    let (registry_tx, registry_rx) = tokio::sync::oneshot::channel::<Arc<ProcessRegistry>>();
    let (hub_tx, hub_rx) = tokio::sync::oneshot::channel::<SubscriptionCreations>();
    let options = crewd_server::ServeOptions {
        test_registry_ready: Some(Box::new(move |registry| {
            registry.test_configure_enrollment_control(control)?;
            registry_tx
                .send(registry)
                .map_err(|_| io::Error::other("registry witness dropped"))
        })),
        test_subscriptions_ready: Some(Box::new(move |counter| {
            let _ = hub_tx.send(counter);
        })),
        ..Default::default()
    };
    let control = f.root.path().join("control");
    let (stop, rx) = tokio::sync::oneshot::channel();
    let mut stop = Some(stop);
    let mut server = tokio::spawn(crewd_server::serve_with(f.data.clone(), options, async {
        let _ = rx.await;
    }));
    let mut child: Option<tokio::process::Child> = None;
    let mut enrollment: Option<Enrollment> = None;
    let observed = Catch(Box::pin(async {
        let registry = tokio::time::timeout(Duration::from_secs(15), registry_rx)
            .await
            .map_err(io::Error::other)?
            .map_err(io::Error::other)?;
        let hub = tokio::time::timeout(Duration::from_secs(15), hub_rx)
            .await
            .map_err(io::Error::other)?
            .map_err(io::Error::other)?;
        let positive_dir = start_child(
            &f,
            "positive-before",
            "crew.create",
            json!({"title":"S1_STAGE3_POSITIVE","workingDirectory":f.work}),
            true,
            &mut child,
        )?;
        let positive = finish_child(&mut child, &positive_dir).await?;
        enrollment = Some(registry.begin_enrollment()?);
        let held = registry.authority_snapshot()?;
        let method = if drain_case {
            "crew.create"
        } else {
            "events.subscribe"
        };
        let params = if drain_case {
            json!({"title":"S1_STAGE3_DENIED","workingDirectory":f.work})
        } else {
            json!({"topics":["crew"]})
        };
        let pending_dir = start_child(&f, "pending", method, params, !drain_case, &mut child)?;
        let pid = child
            .as_ref()
            .and_then(tokio::process::Child::id)
            .ok_or_else(|| io::Error::other("owned pending PID"))?;
        let pending = pending_path(&control, pid, method);
        wait_path(&pending).await?;
        let pending_receipt = read_json(&pending)?;
        let before = selected_rows(&f)?;
        let receivers_before = hub.count();
        let early = child
            .as_mut()
            .ok_or_else(|| io::Error::other("owned child lost"))?
            .try_wait()?;
        let mut revisions = Vec::new();
        if drain_case {
            stop.take()
                .ok_or_else(|| io::Error::other("shutdown sender missing"))?
                .send(())
                .map_err(|_| io::Error::other("shutdown receiver gone"))?;
        } else {
            // Real count/revision changes, while the original guard stays live. No notification grants authority.
            for _ in 0..8 {
                let extra = registry.begin_enrollment()?;
                drop(extra);
                revisions.push(registry.authority_snapshot()?.revision);
            }
        }
        let denied = finish_child(&mut child, &pending_dir).await?;
        let still_held = registry.authority_snapshot()?;
        let denied_rows = selected_rows(&f)?;
        let receivers_after = hub.count();
        let after_positive = if drain_case {
            None
        } else {
            drop(enrollment.take());
            let dir = start_child(
                &f,
                "positive-after",
                "crew.create",
                json!({"title":"S1_STAGE3_AFTER_SETTLEMENT","workingDirectory":f.work}),
                true,
                &mut child,
            )?;
            Some(finish_child(&mut child, &dir).await?)
        };
        let after_rows = selected_rows(&f)?;
        Ok::<_, io::Error>((
            positive,
            held,
            pid,
            pending_receipt,
            before,
            receivers_before,
            early,
            revisions,
            denied,
            still_held,
            denied_rows,
            receivers_after,
            after_positive,
            after_rows,
            hub,
        ))
    }))
    .await;
    // Keep unresolved guard alive until actual connection/writer/storage shutdown has finished.
    if let Some(stop) = stop.take() {
        let _ = stop.send(());
    }
    let closed = tokio::time::timeout(Duration::from_secs(20), &mut server).await;
    let normal = matches!(&closed, Ok(Ok(Ok(()))));
    if closed.is_err() {
        eprintln!("S1 Stage3 serve timeout; owned abort fallback");
        server.abort();
        let _ = server.await;
    }
    let leftover = if let Some(mut child) = child.take() {
        match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
            Ok(status) => Some(format!(
                "original observation did not collect child receipt, later wait={status:?}"
            )),
            Err(error) => {
                let _ = child.start_kill();
                let waited = tokio::time::timeout(Duration::from_secs(15), child.wait()).await;
                Some(format!("owned child timeout fallback {error}: {waited:?}"))
            }
        }
    } else {
        None
    };
    drop(enrollment.take());
    let socket_gone = !f.data.join("crewd.sock").exists();
    let final_rows = selected_rows(&f);
    match observed {
        Err(p) => {
            if !normal || leftover.is_some() {
                eprintln!(
                    "S1 Stage3 cleanup after original panic: normal={normal}, leftover={leftover:?}"
                );
            }
            std::panic::resume_unwind(p)
        }
        Ok(data) => {
            let (
                positive,
                held,
                pid,
                pending,
                before,
                sub_before,
                early,
                revisions,
                denied,
                still_held,
                denied_rows,
                sub_after,
                after_positive,
                after_rows,
                hub,
            ) = data.expect("actual setup/observation before functional classification");
            assert!(
                normal && socket_gone && leftover.is_none(),
                "normal conn/writer/serve/storage/child cleanup before functional oracles"
            );
            assert!(
                positive.0.success()
                    && positive.1["eof"] == true
                    && response(&positive.1).get("result").is_some(),
                "actual external Human positive before hold"
            );
            assert_eq!(held.enrolling, 1);
            assert_eq!(pending["identity"]["pid"], pid);
            assert_eq!(pending["currentParent"], std::process::id());
            assert_eq!(
                pending["method"],
                if drain_case {
                    "crew.create"
                } else {
                    "events.subscribe"
                }
            );
            assert_eq!(before, (1, 0, 0));
            assert_eq!(sub_before, 0);
            assert!(
                early.is_none(),
                "true pending future observed before drain/deadline"
            );
            assert!(
                denied.0.success(),
                "raw wire helper closes normally after response/EOF: status={}, receipt={}",
                denied.0,
                denied.1
            );
            assert_eq!(denied.1["eof"], true, "actual server writer EOF observed");
            assert_eq!(
                denied.1["error"],
                Value::Null,
                "no wire read error substituted for EOF"
            );
            let reply = response(&denied.1);
            assert_eq!(reply["error"]["code"], crewd_proto::RpcError::FORBIDDEN);
            let message = reply["error"]["message"].as_str().unwrap();
            if drain_case {
                assert!(
                    message.contains("server_draining"),
                    "drain refusal rather than eventual auth expiry"
                );
                assert_eq!(
                    denied.1["halfClosed"], false,
                    "drain closes server side without caller half-close"
                );
            } else {
                assert!(message.contains("spawn_enrollment_in_progress"));
                let ms = denied.1["responseElapsedMs"].as_u64().unwrap();
                assert!(
                    (950..3000).contains(&ms),
                    "actual held auth deadline without renewal, elapsed={ms}ms"
                );
                assert_eq!(
                    denied.1["halfClosed"], true,
                    "own client half-close triggers normal connection EOF after exact refusal"
                );
                assert_eq!(revisions.len(), 8);
                assert!(revisions.windows(2).all(|w| w[0] < w[1]));
                let p = after_positive.unwrap();
                assert!(
                    p.0.success() && p.1["eof"] == true && response(&p.1).get("result").is_some(),
                    "fresh external Human works after actual guard retirement"
                );
            }
            assert_eq!(
                still_held.enrolling, 1,
                "no count0/retry or fabricated settlement before original denial"
            );
            assert_eq!(denied_rows, (1, 0, 0));
            assert_eq!(sub_after, 0);
            assert_eq!(
                hub.count(),
                0,
                "no ordinary client Hub receiver created, even if subsequently dropped"
            );
            let expected = if drain_case { (1, 0, 0) } else { (1, 0, 1) };
            assert_eq!(after_rows, expected);
            assert_eq!(final_rows.unwrap(), expected);
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s1_physical_pending_ordinary_drain_returns_original_id_and_server_eof_without_effects() {
    run_case(true).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s1_physical_permanent_enrollment_subscription_has_bounded_denial_without_hub_effects() {
    run_case(false).await;
}
