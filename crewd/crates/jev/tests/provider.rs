mod support;
use crewd_jev::client::ApiKey;
use crewd_jev::provider::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use support::{FakeJev, Reply};
fn batch(version: &str) -> Batch {
    Batch {
        state_version: version.into(),
        state: json!({"text":"original task"}),
        questions: BTreeMap::from([
            (
                "route".into(),
                Judgment::Choice {
                    instructions: json!("choose"),
                    candidates: vec![("s1".into(), json!("one")), ("s2".into(), json!("two"))],
                },
            ),
            (
                "wake_s1".into(),
                Judgment::Predicate {
                    instructions: json!("wake?"),
                    yes: json!("yes"),
                    no: json!("no"),
                },
            ),
            (
                "score".into(),
                Judgment::Score {
                    instructions: json!("level"),
                    levels: vec![json!("low"), json!("high")],
                },
            ),
        ]),
    }
}
fn body() -> String {
    json!({"model":"jev-recorded","answers":{"route":{"type":"choice","choice":"s2","confidence":0.8,"probabilities":{"s1":0.2,"s2":0.8}},"wake_s1":{"type":"noul","noul":0.7},"score":{"type":"score","score":0.9,"confidence":0.6,"probabilities":{"0":0.1,"1":0.9}}},"usage":{"input_tokens":12,"output_tokens":3,"vendor_cache":9}}).to_string()
}
fn provider(fake: &FakeJev) -> DecisionProvider {
    DecisionProvider::new(
        DecisionConfig {
            base_url: fake.base_url.clone(),
            ..Default::default()
        },
        ApiKey::new("fake-owned-key"),
    )
    .unwrap()
}
#[tokio::test]
async fn typed_batch_singleflight_preserves_usage_and_state_invalidation() {
    let fake = FakeJev::start(vec![Reply::ok(body()), Reply::ok(body())]).await;
    let p = Arc::new(provider(&fake));
    let b = batch("version1");
    let (one, two) = tokio::join!(p.evaluate(&b), p.evaluate(&b));
    assert_eq!(one, two);
    match one {
        JudgmentResult::Answered {
            raw_usage,
            calibrated,
            suggestion_only,
            answers,
            ..
        } => {
            assert_eq!(raw_usage.unwrap()["vendor_cache"], 9);
            assert!(!calibrated);
            assert!(!suggestion_only);
            assert_eq!(answers.len(), 3);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fake.requests().len(), 1);
    p.evaluate(&batch("version2")).await;
    assert_eq!(fake.requests().len(), 2);
    let wire: serde_json::Value = serde_json::from_slice(&fake.requests()[0].body).unwrap();
    assert!(wire.get("state_version").is_none());
    assert_eq!(wire["questions"]["wake_s1"]["type"], "noul");
}
#[tokio::test]
async fn invalid_type_or_foreign_key_is_deferred_not_adopted() {
    for answer in [
        json!({"type":"choice","choice":"foreign","confidence":1.0,"probabilities":{"foreign":1.0}}),
        json!({"type":"noul","noul":1.0}),
    ] {
        let mut raw: serde_json::Value = serde_json::from_str(&body()).unwrap();
        raw["answers"]["route"] = answer;
        let fake = FakeJev::start(vec![Reply::ok(raw.to_string())]).await;
        assert!(
            matches!(provider(&fake).evaluate(&batch("one")).await,JudgmentResult::Deferred{reason,..} if reason=="invalid_response")
        );
    }
}
#[tokio::test]
async fn deadline_bounds_retries_and_no_provider_fallback() {
    let fake = FakeJev::start(vec![Reply::status(429).header("Retry-After", "8")]).await;
    assert!(matches!(
        provider(&fake).evaluate(&batch("one")).await,
        JudgmentResult::Deferred { .. }
    ));
    assert_eq!(fake.requests().len(), 1);
    let fake = FakeJev::start(vec![
        Reply::ok(body()).delayed(std::time::Duration::from_secs(4)),
    ])
    .await;
    let started = std::time::Instant::now();
    assert!(matches!(
        provider(&fake).evaluate(&batch("two")).await,
        JudgmentResult::Deferred { .. }
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    assert_eq!(fake.requests().len(), 1);
}
#[tokio::test]
async fn unconfigured_and_invalid_batches_never_send() {
    let fake = FakeJev::start(vec![]).await;
    let p = DecisionProvider::new(
        DecisionConfig {
            base_url: fake.base_url.clone(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert!(
        matches!(p.evaluate(&batch("one")).await,JudgmentResult::Deferred{reason,..} if reason=="unconfigured")
    );
    let p = provider(&fake);
    let mut b = batch("");
    assert!(matches!(
        p.evaluate(&b).await,
        JudgmentResult::Deferred { .. }
    ));
    b.state_version = "v".into();
    b.questions.insert(
        "route".into(),
        Judgment::Choice {
            instructions: json!(""),
            candidates: vec![("same".into(), json!(1)), ("same".into(), json!(2))],
        },
    );
    assert!(matches!(
        p.evaluate(&b).await,
        JudgmentResult::Deferred { .. }
    ));
    assert!(fake.requests().is_empty());
}
#[tokio::test]
async fn canceled_waiter_cannot_dispatch_the_same_state_again() {
    let fake = FakeJev::start(vec![
        Reply::ok(body()).delayed(std::time::Duration::from_millis(80)),
    ])
    .await;
    let p = Arc::new(provider(&fake));
    let first = p.clone();
    let task = tokio::spawn(async move { first.evaluate(&batch("same")).await });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while fake.requests().is_empty() {
            tokio::task::yield_now().await
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    assert!(matches!(
        p.evaluate(&batch("same")).await,
        JudgmentResult::Answered { .. }
    ));
    assert_eq!(fake.requests().len(), 1);
}
#[tokio::test]
async fn oversized_response_is_deferred_without_retries() {
    let fake = FakeJev::start(vec![Reply::ok("x".repeat(1024 * 1024 + 1))]).await;
    assert!(matches!(
        provider(&fake).evaluate(&batch("one")).await,
        JudgmentResult::Deferred { .. }
    ));
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn revoke_cancels_dispatched_instance_and_future_calls_without_replay() {
    let fake = FakeJev::start(vec![Reply {
        delay: std::time::Duration::from_secs(4),
        ..Reply::ok(body())
    }])
    .await;
    let p = Arc::new(provider(&fake));
    let p1 = p.clone();
    let waiting = tokio::spawn(async move { p1.evaluate(&batch("revision-before-change")).await });
    for _ in 0..100 {
        if !fake.requests().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(fake.requests().len(), 1);
    p.revoke();
    let result = tokio::time::timeout(std::time::Duration::from_millis(200), waiting)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result,JudgmentResult::Deferred{reason,..} if reason=="configuration_changed")
    );
    assert!(
        matches!(p.evaluate(&batch("new-version")).await,JudgmentResult::Deferred{reason,..} if reason=="configuration_changed")
    );
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn shortened_absolute_deadline_cancels_owner_and_is_cached_without_replay() {
    let fake = FakeJev::start(vec![
        Reply::ok(body()).delayed(std::time::Duration::from_millis(500)),
    ])
    .await;
    let p = provider(&fake);
    let started = std::time::Instant::now();
    let result = p
        .evaluate_until(
            &batch("one-budget"),
            started + std::time::Duration::from_millis(50),
        )
        .await;
    assert!(matches!(result,JudgmentResult::Deferred{reason,..} if reason=="deadline"));
    assert!(started.elapsed() < std::time::Duration::from_millis(250));
    let again = p.evaluate(&batch("one-budget")).await;
    assert!(matches!(again,JudgmentResult::Deferred{reason,..} if reason=="deadline"));
    assert_eq!(fake.requests().len(), 1);
}
