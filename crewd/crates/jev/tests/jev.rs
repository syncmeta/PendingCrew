//! 门面对着假 Jev：未配置不联网、端到端判定、诊断日志（不含 key、摘要已脱敏）。
mod support;

use crewd_jev::client::{ApiKey, Client, ClientConfig, RetryPolicy};
use crewd_jev::decisions::route::{RouteTarget, RouteTodoInput};
use crewd_jev::decisions::wake::{WakeInput, WakeSet};
use crewd_jev::decisions::{DeferReason, Member, Outcome};
use crewd_jev::diag::DiagLog;
use crewd_jev::error::CallError;
use crewd_jev::jev::{Jev, JevConfig};
use serde_json::Value;
use std::time::Duration;
use support::{FakeJev, Reply, choice_body, noul_body};

const TEST_KEY: &str = "test-key-not-real-0123456789";

fn member(key: &str, rank: usize) -> Member {
    Member {
        key: key.into(),
        name: format!("成员{key}"),
        role: format!("负责 {key}"),
        work: vec![],
        busy: false,
        recency_rank: rank,
    }
}

fn route_input(todo: &str) -> RouteTodoInput {
    RouteTodoInput {
        todo: todo.into(),
        members: vec![member("scroll", 1), member("release", 2)],
    }
}

fn jev_against(base: &str, diag: Option<DiagLog>) -> Jev {
    let mut cfg = ClientConfig::new(base, ApiKey::new(TEST_KEY).unwrap());
    cfg.retry = RetryPolicy {
        max_attempts: 2,
        base_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(20),
    };
    cfg.attempt_timeout = Duration::from_millis(500);
    cfg.deadline = Duration::from_secs(2);
    Jev::with_client(Some(Client::new(cfg).unwrap()), &JevConfig::default(), diag)
}

fn diag_lines(log: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
async fn without_a_key_nothing_leaves_and_everything_defers() {
    let fake = FakeJev::start(vec![]).await;
    let mut config = JevConfig {
        base_url: fake.base_url.clone(),
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let diag = DiagLog::open(dir.path().join("jev.jsonl")).unwrap();
    let jev = Jev::new(&config, None, Some(diag)).unwrap();
    assert!(!jev.is_configured());
    let v = jev.route_todo(&route_input("修滚动")).await;
    assert_eq!(v.outcome, Outcome::Defer(DeferReason::Unconfigured));
    config.enabled = false;
    let off = Jev::new(&config, ApiKey::new(TEST_KEY), None).unwrap();
    assert!(!off.is_configured(), "关掉了，有 key 也不发");
    assert_eq!(fake.requests().len(), 0);
    let lines = diag_lines(&dir.path().join("jev.jsonl"));
    assert_eq!(lines[0]["outcome"], "defer");
    assert_eq!(lines[0]["reason"], "unconfigured");
}

#[test]
fn bad_base_url_is_reported_not_silently_unconfigured() {
    let config = JevConfig {
        base_url: "http://api.typesafe.ai".into(),
        ..Default::default()
    };
    assert!(Jev::new(&config, ApiKey::new(TEST_KEY), None).is_err());
}

#[tokio::test]
async fn route_acts_on_a_confident_answer_and_logs_without_the_key() {
    let fake = FakeJev::start(vec![Reply::ok(choice_body("route", "scroll", 0.9))]).await;
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("jev.jsonl");
    let jev = jev_against(&fake.base_url, Some(DiagLog::open(&log).unwrap()));
    let v = jev.route_todo(&route_input("终端滚动跳动")).await;
    assert_eq!(
        v.outcome,
        Outcome::Act(RouteTarget::Member("scroll".into()))
    );
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(!text.contains(TEST_KEY), "诊断日志里出现了 key");
    assert!(!text.to_ascii_lowercase().contains("bearer"));
    let line = &diag_lines(&log)[0];
    assert_eq!(line["decision"], "route_todo");
    assert_eq!(line["outcome"], "act");
    assert_eq!(line["confidence"], 0.9);
    assert_eq!(line["answer"]["key"], "scroll");
    assert_eq!(line["options"].as_array().unwrap().len(), 5);
    assert!(
        line["state_excerpt"]
            .as_str()
            .unwrap()
            .contains("终端滚动跳动")
    );
    assert_eq!(jev.diag_failures(), 0);
}

#[tokio::test]
async fn secrets_are_scrubbed_from_the_request_and_the_log() {
    let secret = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
    let fake = FakeJev::start(vec![Reply::ok(choice_body("route", "release", 0.9))]).await;
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("jev.jsonl");
    let jev = jev_against(&fake.base_url, Some(DiagLog::open(&log).unwrap()));
    jev.route_todo(&route_input(&format!("CI 用的 token 换成 {secret}")))
        .await;
    assert!(!fake.requests()[0].body_text().contains(&secret));
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(!text.contains(&secret));
    assert_eq!(diag_lines(&log)[0]["scrub_hits"], 1);
}

#[tokio::test]
async fn low_confidence_and_errors_defer_to_the_captain() {
    let fake = FakeJev::start(vec![
        Reply::ok(choice_body("route", "captain", 0.2)),
        Reply::status(401),
    ])
    .await;
    let jev = jev_against(&fake.base_url, None);
    let v = jev.route_todo(&route_input("排一下优先级")).await;
    assert_eq!(v.best, Some(RouteTarget::Captain));
    assert_eq!(
        v.outcome,
        Outcome::Defer(DeferReason::LowConfidence { confidence: 0.2 })
    );
    let v = jev.route_todo(&route_input("再排一下")).await;
    assert_eq!(
        v.outcome,
        Outcome::Defer(DeferReason::Call(CallError::Unauthorized { status: 401 }))
    );
}

#[tokio::test]
async fn invalid_input_never_reaches_the_network() {
    let fake = FakeJev::start(vec![]).await;
    let jev = jev_against(&fake.base_url, None);
    let v = jev.route_todo(&route_input("   ")).await;
    assert!(matches!(
        v.outcome,
        Outcome::Defer(DeferReason::InvalidInput(_))
    ));
    assert_eq!(fake.requests().len(), 0);
}

#[tokio::test]
async fn wake_end_to_end() {
    let fake = FakeJev::start(vec![Reply::ok(noul_body(&[
        ("scroll", 0.92),
        ("release", 0.08),
    ]))])
    .await;
    let jev = jev_against(&fake.base_url, None);
    let v = jev
        .wake(&WakeInput {
            message: "试了，还是会跳".into(),
            recent: vec!["滚动修复：改好了你试试".into()],
            members: vec![member("scroll", 1), member("release", 2)],
        })
        .await;
    assert_eq!(
        v.outcome,
        Outcome::Act(WakeSet {
            wake: vec!["scroll".into()]
        })
    );
    let body: Value = serde_json::from_slice(&fake.requests()[0].body).unwrap();
    assert_eq!(body["questions"]["scroll"]["type"], "noul");
    assert_eq!(body["state"]["message"], "试了，还是会跳");
}
