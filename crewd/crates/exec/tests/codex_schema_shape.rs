//! Offline shape contract extracted from the installed 0.162.1 CLI's
//! generate-json-schema --experimental output. No model or credentials involved.
use crewd_exec::{
    codex::proto::{self, ThreadOptions},
    driver::{Control, UntrustedContext},
};
use serde_json::{Value, json};
use std::path::Path;

#[test]
fn outbound_methods_and_fields_match_the_native_experimental_contract() {
    let schema: Value =
        serde_json::from_str(include_str!("fixtures/codex-0.162.1-client-shape.json")).unwrap();
    let options = ThreadOptions {
        model: Some("synthetic-model".into()),
        effort: Some("high".into()),
        fast: Some(true),
        developer_instructions: Some("synthetic worldview".into()),
        mcp_servers: Some(json!({"crew":{"command":"/synthetic/helper","args":[]}})),
    };
    let context = UntrustedContext::from([("crew_whiteboard".into(), "external data".into())]);
    let inputs = [
        ("initialize", proto::initialize_params("synthetic")),
        (
            "thread/start",
            proto::thread_start_params(Path::new("/synthetic"), &options),
        ),
        (
            "thread/resume",
            proto::thread_resume_params("thread", Path::new("/synthetic"), &options),
        ),
        (
            "turn/start",
            proto::turn_start_params("thread", "task", &context),
        ),
        (
            "turn/interrupt",
            proto::turn_interrupt_params("thread", "turn"),
        ),
        (
            "thread/settings/update",
            proto::settings_update_params("thread", &Control::Fast(true)).unwrap(),
        ),
        ("thread/compact/start", proto::compact_params("thread")),
    ];
    for (method, params) in inputs {
        let keys = schema["methodParams"][method]
            .as_array()
            .unwrap_or_else(|| panic!("native method missing: {method}"));
        for key in params.as_object().unwrap().keys() {
            assert!(
                keys.contains(&json!(key)),
                "native method {method} does not accept {key}"
            );
        }
    }
    let init = proto::initialize_params("synthetic");
    for key in init["capabilities"].as_object().unwrap().keys() {
        assert!(
            schema["initializeCapabilities"]
                .as_array()
                .unwrap()
                .contains(&json!(key)),
            "native capability missing: {key}"
        );
    }
    let turn = proto::turn_start_params("thread", "task", &context);
    for key in turn["additionalContext"]["crew_whiteboard"]
        .as_object()
        .unwrap()
        .keys()
    {
        assert!(
            schema["additionalContextEntry"]
                .as_array()
                .unwrap()
                .contains(&json!(key)),
            "native context entry missing: {key}"
        );
    }
    assert_eq!(
        turn["additionalContext"]["crew_whiteboard"]["kind"],
        "untrusted"
    );
    assert_eq!(
        init["capabilities"]["experimentalApi"], true,
        "these fields require explicit experimental opt-in"
    );
}

#[test]
fn trusted_crew_preserves_input_schemas_on_start_and_resume() {
    let options = ThreadOptions {
        mcp_servers: Some(proto::mcp_servers(
            Path::new("/synthetic/helper"),
            &["mcp".into()],
        )),
        ..Default::default()
    };
    for params in [
        proto::thread_start_params(Path::new("/synthetic"), &options),
        proto::thread_resume_params("thread", Path::new("/synthetic"), &options),
    ] {
        let servers = params["config"]["mcp_servers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers["crew"]["tool_input_schema_max_bytes"], 16_384);
        assert_eq!(servers["crew"]["command"], "/synthetic/helper");
        assert_eq!(servers["crew"]["args"], json!(["mcp"]));
    }
}
