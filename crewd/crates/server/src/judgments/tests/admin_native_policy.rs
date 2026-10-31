//! Actual Core boundary: native capability refusal precedes case registration.
use super::*;
use crate::orchestra::exec::NativeExecutionPolicy;

#[tokio::test]
async fn unsupported_restricted_executor_refuses_before_registration_but_allows_ordinary_admin() {
    let (f, source, permit, _fake) = prepared().await;
    let r = Rig::new(f).await;
    r.exec.lock().reject_admin_case_policy = true;
    let refused = r.handle.start_admin_case(permit).await;
    assert!(
        refused.is_err(),
        "unsupported case policy must refuse before binding"
    );
    r.handle.latches().await.unwrap();
    assert!(view(&r.f, &source.id).await["executionRecord"].is_null());
    let count =
        r.f.state
            .db
            .call_result(|s| {
                s.conn().query_row(
                    "SELECT count(*) FROM sessions WHERE role='admin'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        count, 0,
        "no orphan Admin registration on unsupported native policy"
    );
    let ordinary = r
        .handle
        .start(
            orchestra::core::StartSession {
                crew_id: r.f.crew.clone(),
                role: Role::Admin,
                executor: "claude".into(),
                brief: "ordinary Human-created administrator".into(),
                title: None,
                model: None,
                effort: None,
                isolation: false,
                codex_options: None,
            },
            orchestra::core::Origin::Human,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while r.exec.launches(&ordinary.id).is_empty() {
            r.handle.latches().await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        r.exec.launches(&ordinary.id)[0].native_policy,
        NativeExecutionPolicy::Ordinary
    );
    r.close().await;
}

#[tokio::test]
async fn actual_bound_admin_launch_receives_restricted_policy() {
    let (f, source, permit, _fake) = prepared().await;
    let r = Rig::new(f).await;
    let session = r.handle.start_admin_case(permit).await.unwrap();
    wait_started(&r, &source.id, &session.id).await;
    let launches = r.exec.launches(&session.id);
    assert_eq!(launches.len(), 1);
    assert_eq!(
        launches[0].native_policy,
        NativeExecutionPolicy::AdminCaseRestricted
    );
    r.close().await;
}
