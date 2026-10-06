//! 客户端对着假 Jev：请求形状、重试、超时、脱敏、选项顺序。
mod support;

use crewd_jev::client::{ApiKey, Client, ClientConfig, RetryPolicy};
use crewd_jev::error::CallError;
use crewd_jev::model::{ChoiceCriteria, Question, Request};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use support::{FakeJev, Reply, choice_body};

const TEST_KEY: &str = "test-key-not-real-0123456789";

fn config(base: &str) -> ClientConfig {
    let mut c = ClientConfig::new(base, ApiKey::new(TEST_KEY).unwrap());
    c.retry = RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(50),
    };
    c.attempt_timeout = Duration::from_millis(300);
    c.deadline = Duration::from_secs(3);
    c
}

fn request(state: Value) -> Request {
    let mut questions = BTreeMap::new();
    questions.insert(
        "route".to_string(),
        Question::Choice {
            instructions: json!("谁来做"),
            criteria: ChoiceCriteria(vec![
                ("zeta".into(), json!("z")),
                ("alpha".into(), json!("a")),
                ("mid".into(), json!("m")),
            ]),
        },
    );
    Request {
        model: "jev-latest".into(),
        state,
        questions,
    }
}

#[tokio::test]
async fn posts_json_with_bearer_to_v1_systemone() {
    let fake = FakeJev::start(vec![Reply::ok(choice_body("route", "alpha", 0.9))]).await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({"todo": "修滚动"}))).await;
    let response = result.unwrap();
    assert_eq!(response.model, "jev-1.13.0");
    assert_eq!(report.attempts, 1);
    let reqs = fake.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(reqs[0].path, "/v1/systemone");
    assert_eq!(
        reqs[0].header("authorization"),
        Some(&*format!("Bearer {TEST_KEY}"))
    );
    assert_eq!(reqs[0].header("content-type"), Some("application/json"));
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["state"]["todo"], "修滚动");
    assert_eq!(body["questions"]["route"]["type"], "choice");
}

#[tokio::test]
async fn option_order_survives_the_wire() {
    let fake = FakeJev::start(vec![Reply::ok(choice_body("route", "alpha", 0.9))]).await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    client.evaluate(&request(json!({}))).await.0.unwrap();
    let text = fake.requests()[0].body_text();
    let (z, a, m) = (
        text.find("\"zeta\"").unwrap(),
        text.find("\"alpha\"").unwrap(),
        text.find("\"mid\"").unwrap(),
    );
    assert!(z < a && a < m, "选项顺序被重排了：{text}");
}

#[tokio::test]
async fn secrets_never_leave_the_machine() {
    let secret = "sk-ant-api03-AbCdEf0123456789xyzXYZ_-abcdef";
    let fake = FakeJev::start(vec![Reply::ok(choice_body("route", "alpha", 0.9))]).await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (_, report) = client
        .evaluate(&request(
            json!({"todo": format!("用这个 key 测一下 {secret}")}),
        ))
        .await;
    assert_eq!(report.scrub_hits, 1);
    let text = fake.requests()[0].body_text();
    assert!(!text.contains(secret), "{text}");
    assert!(text.contains("[REDACTED]"));
}

#[tokio::test]
async fn rate_limit_and_overload_are_retried_then_succeed() {
    let fake = FakeJev::start(vec![
        Reply::status(429),
        Reply::status(529),
        Reply::ok(choice_body("route", "mid", 0.8)),
    ])
    .await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(report.attempts, 3);
    assert_eq!(fake.requests().len(), 3);
}

#[tokio::test]
async fn gives_up_after_max_attempts() {
    let fake = FakeJev::start(vec![
        Reply::status(429),
        Reply::status(429),
        Reply::status(429),
        Reply::status(429),
    ])
    .await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert_eq!(result.unwrap_err(), CallError::RateLimited);
    assert_eq!(report.attempts, 3);
    assert_eq!(fake.requests().len(), 3, "第四个应答不该被取");
}

#[tokio::test]
async fn retry_after_is_honoured_but_capped() {
    let fake = FakeJev::start(vec![
        Reply::status(429).header("retry-after", "30"),
        Reply::ok(choice_body("route", "zeta", 0.7)),
    ])
    .await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let started = Instant::now();
    let (result, _) = client.evaluate(&request(json!({}))).await;
    assert!(result.is_ok());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "Retry-After 30s 应被封顶到 max_delay"
    );
}

#[tokio::test]
async fn unauthorized_and_rejected_are_not_retried() {
    let fake = FakeJev::start(vec![Reply::status(401)]).await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert_eq!(result.unwrap_err(), CallError::Unauthorized { status: 401 });
    assert_eq!(report.attempts, 1);

    let fake = FakeJev::start(vec![
        Reply::status(422).body("{\"detail\":\"questions.route.criteria\"}"),
    ])
    .await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    match result.unwrap_err() {
        CallError::Rejected { status, body } => {
            assert_eq!(status, 422);
            assert!(body.contains("questions.route.criteria"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(report.attempts, 1);
}

#[tokio::test]
async fn malformed_success_is_not_retried() {
    let fake = FakeJev::start(vec![Reply::ok("not json")]).await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert!(matches!(result, Err(CallError::Malformed(_))), "{result:?}");
    assert_eq!(report.attempts, 1);
}

#[tokio::test]
async fn a_slow_attempt_times_out_and_the_retry_succeeds() {
    let fake = FakeJev::start(vec![
        Reply::ok(choice_body("route", "zeta", 0.9)).delayed(Duration::from_secs(2)),
        Reply::ok(choice_body("route", "zeta", 0.9)),
    ])
    .await;
    let client = Client::new(config(&fake.base_url)).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(report.attempts, 2);
}

#[tokio::test]
async fn the_deadline_bounds_the_whole_call() {
    let slow = || Reply::ok(choice_body("route", "zeta", 0.9)).delayed(Duration::from_secs(5));
    let fake = FakeJev::start(vec![slow(), slow(), slow()]).await;
    let mut cfg = config(&fake.base_url);
    cfg.attempt_timeout = Duration::from_secs(10);
    cfg.deadline = Duration::from_millis(400);
    let client = Client::new(cfg).unwrap();
    let started = Instant::now();
    let (result, _) = client.evaluate(&request(json!({}))).await;
    assert_eq!(result.unwrap_err(), CallError::Timeout);
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn nothing_listening_is_a_transport_error_after_retries() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let client = Client::new(config(&format!("http://127.0.0.1:{port}"))).unwrap();
    let (result, report) = client.evaluate(&request(json!({}))).await;
    assert!(matches!(result, Err(CallError::Transport(_))), "{result:?}");
    assert_eq!(report.attempts, 3);
}
