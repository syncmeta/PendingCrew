//! jev-eval：用真 API 跑中文实测集，报准确率、按置信度分桶、推荐阈值。
//! opt-in：check.sh 不跑它，CI 也不跑。会花钱（每条用例 × 每个种子一次请求）。
//!
//! key：环境变量 JEV_API_KEY；没有就在 macOS 上用 `security find-generic-password` 读钥匙串
//! （service=pendingcrew-jev，account=default）。key 不打印、不落盘。
//!
//! 用法：cargo run -p crewd-jev --bin jev-eval -- [--cases PATH] [--seeds N] [--only route_todo|wake|attribute|urgency]
//!       [--out REPORT.json] [--record DIR] [--base-url URL] [--model NAME] [--target 0.95]
use crewd_jev::client::{ApiKey, Client, ClientConfig};
use crewd_jev::eval::{Dataset, Judged, judge, summarize};
use crewd_jev::key::{KEYCHAIN_ACCOUNT, KEYCHAIN_SERVICE};
use crewd_jev::model::{DEFAULT_BASE_URL, DEFAULT_MODEL};
use crewd_jev::shuffle::seed_from;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const DEFAULT_CASES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/eval/cases.zh.json");
/// 两次请求之间歇一下，别自己把自己限流。
const PAUSE: Duration = Duration::from_millis(200);

struct Args {
    cases: PathBuf,
    seeds: u64,
    only: Option<String>,
    out: Option<PathBuf>,
    record: Option<PathBuf>,
    base_url: String,
    model: String,
    target: f64,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        cases: PathBuf::from(DEFAULT_CASES),
        seeds: 2,
        only: None,
        out: None,
        record: None,
        base_url: DEFAULT_BASE_URL.into(),
        model: DEFAULT_MODEL.into(),
        target: 0.95,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} 缺参数"));
        match flag.as_str() {
            "--cases" => args.cases = PathBuf::from(value()?),
            "--seeds" => args.seeds = value()?.parse().map_err(|e| format!("--seeds：{e}"))?,
            "--only" => args.only = Some(value()?),
            "--out" => args.out = Some(PathBuf::from(value()?)),
            "--record" => args.record = Some(PathBuf::from(value()?)),
            "--base-url" => args.base_url = value()?,
            "--model" => args.model = value()?,
            "--target" => args.target = value()?.parse().map_err(|e| format!("--target：{e}"))?,
            other => return Err(format!("不认识的参数 {other}")),
        }
    }
    if args.seeds == 0 {
        return Err("--seeds 至少 1".into());
    }
    Ok(args)
}

fn read_key() -> Result<ApiKey, String> {
    if let Ok(v) = std::env::var("JEV_API_KEY") {
        return ApiKey::new(&v).ok_or_else(|| "JEV_API_KEY 是空的或含空白".to_string());
    }
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                KEYCHAIN_ACCOUNT,
                "-w",
            ])
            .output()
            .map_err(|e| format!("跑 security 失败：{e}"))?;
        if out.status.success() {
            return ApiKey::new(&String::from_utf8_lossy(&out.stdout))
                .ok_or_else(|| "钥匙串里的 key 是空的".to_string());
        }
    }
    Err(format!(
        "没有 key：设 JEV_API_KEY，或在钥匙串里放 service={KEYCHAIN_SERVICE} account={KEYCHAIN_ACCOUNT}"
    ))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jev-eval: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    let text = std::fs::read_to_string(&args.cases)
        .map_err(|e| format!("读 {} 失败：{e}", args.cases.display()))?;
    let dataset = Dataset::parse(&text)?;
    let client = Client::new(ClientConfig::new(args.base_url.clone(), read_key()?))?;
    if let Some(dir) = &args.record {
        std::fs::create_dir_all(dir).map_err(|e| format!("建 {} 失败：{e}", dir.display()))?;
    }
    let cases: Vec<_> = dataset
        .cases
        .iter()
        .filter(|c| args.only.as_deref().is_none_or(|k| c.kind().as_str() == k))
        .collect();
    eprintln!(
        "jev-eval: {} 条用例 × {} 个种子 → {}",
        cases.len(),
        args.seeds,
        client.endpoint()
    );
    let mut judged: Vec<Judged> = Vec::new();
    let mut latencies = Vec::new();
    // jev-latest 是别名；记下实际回答的模型版本（spec 里要写的是它）。
    let mut models = std::collections::BTreeSet::new();
    for case in cases {
        for s in 0..args.seeds {
            let seed = seed_from(&[case.id(), &s.to_string()]);
            let built = dataset.build_case(case, seed)?;
            let (result, report) = client.evaluate_raw(&built.request(&args.model)).await;
            latencies.push(report.latency);
            let j = match result {
                Ok((response, raw)) => {
                    models.insert(response.model.clone());
                    if let Some(dir) = &args.record {
                        // 原文照存：夹具要的是真实应答的形状，不是我们类型再序列化一遍。
                        let path = dir.join(format!("{}-s{s}.json", case.id()));
                        std::fs::write(&path, raw)
                            .map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
                    }
                    judge(case, seed, &built, &response)
                }
                Err(e) => Judged::failed(case, seed, e.to_string()),
            };
            eprintln!(
                "{:<10} s{s} {} {:<24} conf={} {}ms{}",
                j.id,
                if j.correct { "✓" } else { "✗" },
                j.answer,
                j.confidence.map_or("-".into(), |c| format!("{c:.2}")),
                report.latency.as_millis(),
                j.error
                    .as_deref()
                    .map_or(String::new(), |e| format!(" error={e}")),
            );
            judged.push(j);
            tokio::time::sleep(PAUSE).await;
        }
    }
    let summary = summarize(&judged, &latencies, args.target);
    let models: Vec<String> = models.into_iter().collect();
    println!("实际模型：{}\n", models.join(", "));
    println!("{}", summary.markdown());
    if let Some(out) = &args.out {
        let report = serde_json::json!({
            "model": args.model,
            "resolved_models": models,
            "cases": args.cases.file_name().map(|n| n.to_string_lossy().into_owned()),
            "seeds": args.seeds,
            "summary": summary,
            "runs": judged,
        });
        let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        std::fs::write(out, text).map_err(|e| format!("写 {} 失败：{e}", out.display()))?;
    }
    Ok(())
}
