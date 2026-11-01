//! Trusted, host-selected native case policy. Never deserialized from tool input.
use base64::Engine;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NativePolicy {
    #[default]
    Ordinary,
    AdminCaseRestricted,
}
/// Only transport proxies and locale may be supplied to a restricted child.
/// Executable injection (NODE_OPTIONS, BASH_ENV, loader flags, HOME overrides)
/// is rejected before the original-home metadata process is even created.
pub fn permits_additional_env(env: &BTreeMap<String, String>) -> bool {
    env.keys().all(|key| {
        matches!(
            key.as_str(),
            "HTTP_PROXY"
                | "HTTPS_PROXY"
                | "NO_PROXY"
                | "http_proxy"
                | "https_proxy"
                | "no_proxy"
                | "LANG"
        ) || key.starts_with("LC_")
    })
}
pub const PROFILE_ID: &str = "pcw_admin_case_v1";
/// Only access credentials cross the native-source boundary. Never serialize
/// this value into events, persisted metadata, argv, or diagnostic output.
#[derive(Clone, PartialEq, Eq)]
pub struct ExternalChatgptAuth {
    access_token: String,
    account_id: String,
    user_id: String,
    plan: Option<String>,
    expires_at: i64,
}
impl std::fmt::Debug for ExternalChatgptAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExternalChatgptAuth([REDACTED])")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthFailure {
    Unavailable,
    Unsupported,
    Unknown,
    IdentityChanged,
    Expired,
    Unchanged,
}
/// A host-owned reference to the original native authentication owner. This
/// interface cannot create an owner or select a different account/provider.
pub trait NativeAuthSource: Send + Sync {
    fn access(
        &self,
        refresh: bool,
        previous: Option<ExternalChatgptAuth>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ExternalChatgptAuth, AuthFailure>> + Send + '_>,
    >;
}
#[derive(Clone, PartialEq, Eq)]
pub struct SecretLine(pub(crate) String);
impl std::fmt::Debug for SecretLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl ExternalChatgptAuth {
    /// Native legacy getAuthStatus exports the access JWT only. JWT claims here
    /// are consistency checks on the trusted native source, not signature proof.
    pub fn from_native(value: &Value, now: i64) -> Result<Self, AuthFailure> {
        if value["authMethod"] != "chatgpt" || value["requiresOpenaiAuth"] != true {
            return Err(AuthFailure::Unsupported);
        }
        let token = value["authToken"]
            .as_str()
            .filter(|s| s.len() <= 65536)
            .ok_or(AuthFailure::Unavailable)?;
        let parts: Vec<_> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(AuthFailure::Unavailable);
        }
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|_| AuthFailure::Unavailable)?;
        let claims: Value = serde_json::from_slice(&bytes).map_err(|_| AuthFailure::Unavailable)?;
        let identity = &claims["https://api.openai.com/auth"];
        let text = |key| {
            identity[key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let auth = Self {
            access_token: token.to_owned(),
            account_id: text("chatgpt_account_id").ok_or(AuthFailure::Unavailable)?,
            user_id: text("chatgpt_user_id").ok_or(AuthFailure::Unavailable)?,
            plan: text("chatgpt_plan_type"),
            expires_at: claims["exp"].as_i64().ok_or(AuthFailure::Unavailable)?,
        };
        auth.check_valid(now)?;
        Ok(auth)
    }
    pub fn check_valid(&self, now: i64) -> Result<(), AuthFailure> {
        if self.expires_at <= now.saturating_add(30) {
            Err(AuthFailure::Expired)
        } else {
            Ok(())
        }
    }
    pub fn account_id(&self) -> &str {
        &self.account_id
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        self.account_id == other.account_id && self.user_id == other.user_id
    }
    pub fn changed_access(&self, other: &Self) -> bool {
        self.access_token != other.access_token
    }
    pub(crate) fn login_params(&self) -> Value {
        json!({"type":"chatgptAuthTokens","accessToken":self.access_token,"chatgptAccountId":self.account_id,"chatgptPlanType":self.plan})
    }
    pub(crate) fn refresh_result(&self) -> Value {
        json!({"accessToken":self.access_token,"chatgptAccountId":self.account_id,"chatgptPlanType":self.plan})
    }
}
const DISABLED: &[&str] = &[
    "apps",
    "plugins",
    "remote_plugin",
    "browser_use",
    "browser_use_external",
    "browser_use_full_cdp_access",
    "computer_use",
    "multi_agent",
    "realtime_conversation",
    "workspace_dependencies",
    "image_generation",
    "code_mode",
    "code_mode_host",
    "shell_snapshot",
    "hooks",
    "plugin_discovery_prefetch",
    "skill_mcp_dependency_install",
    "in_app_browser",
];
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestrictedProfile {
    pub(crate) model_provider: String,
    pub(crate) provider_fingerprint: String,
    pub(crate) home: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) denied: Vec<PathBuf>,
    pub(crate) external_auth: Option<ExternalChatgptAuth>,
}
fn invalid() -> io::Error {
    io::Error::other("restricted Codex profile unavailable; no task input sent")
}
fn private_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let m = f.metadata()?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.permissions().mode() & 0o077 != 0
        || m.len() > 1024 * 1024
    {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    Ok(bytes)
}
fn create_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}
/// A case owns its native state. Never copy user/project configuration or plugins.
/// Authentication method is separately verified against native effective config.
pub(crate) fn prepare(
    runtime: &Path,
    session: &str,
    cwd: &Path,
    env: &mut BTreeMap<String, String>,
    auth: &ExternalChatgptAuth,
) -> io::Result<RestrictedProfile> {
    if !crate::resume::valid_agent_session_id(session) {
        return Err(invalid());
    }
    let original = env
        .get("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env.get("HOME").map(|h| Path::new(h).join(".codex")))
        .ok_or_else(invalid)?;
    let original = fs::canonicalize(original)?;
    let cwd = fs::canonicalize(cwd)?;
    fs::create_dir_all(runtime)?;
    let runtime = fs::canonicalize(runtime)?;
    let home = runtime.join(format!("codex-case-home-{session}"));
    match fs::symlink_metadata(&home) {
        Ok(m) => {
            if !m.is_dir()
                || m.file_type().is_symlink()
                || m.uid() != unsafe { libc::geteuid() }
                || m.permissions().mode() & 0o077 != 0
            {
                return Err(invalid());
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(&home)?;
            fs::set_permissions(&home, fs::Permissions::from_mode(0o700))?;
        }
        Err(e) => return Err(e),
    }
    let marker = home.join("pcw-case-home.json");
    let expected = json!({"version":2,"session":session,"sourceHome":original,"cwd":cwd,"account":auth.account_id,"user":auth.user_id});
    if marker.exists() {
        let actual: Value =
            serde_json::from_slice(&private_file(&marker)?).map_err(|_| invalid())?;
        if actual != expected {
            return Err(invalid());
        }
        if home.join("auth.json").exists() {
            return Err(invalid());
        }
    } else {
        // A partial prior creation is ambiguous; do not silently replace auth/state.
        if fs::read_dir(&home)?.next().is_some() {
            return Err(invalid());
        }
        create_private(
            &marker,
            &serde_json::to_vec(&expected).map_err(|_| invalid())?,
        )?;
    }
    if home.join("config.toml").exists() || home.join("managed_config.toml").exists() {
        return Err(invalid());
    }
    env.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
    Ok(RestrictedProfile {
        model_provider: String::new(),
        provider_fingerprint: String::new(),
        home,
        cwd,
        denied: vec![runtime, original],
        external_auth: Some(auth.clone()),
    })
}
/// Compare provider routing/auth configuration without exporting credentials.
pub(crate) fn provider_fingerprint(c: &Value) -> Option<String> {
    let id = c["model_provider"].as_str()?;
    let scoped = json!({"id":id,"definition":c["model_providers"][id],"openaiBaseUrl":c["openai_base_url"],"chatgptBaseUrl":c["chatgpt_base_url"],"forcedLoginMethod":c["forced_login_method"],"forcedWorkspace":c["forced_chatgpt_workspace_id"]});
    let bytes = serde_json::to_vec(&scoped).ok()?;
    Some(
        ring::digest::digest(&ring::digest::SHA256, &bytes)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}
fn toml(v: &Value) -> String {
    match v {
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}={}", serde_json::to_string(k).expect("key"), toml(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Array(a) => format!("[{}]", a.iter().map(toml).collect::<Vec<_>>().join(",")),
        _ => v.to_string(),
    }
}
/// Routing discovery has no credentials: ephemeral storage forbids native
/// persistent fallback (including Keychain). It cannot become another refresh
/// owner. Optional integrations are disabled before metadata-only initialize.
pub(crate) fn auth_probe_argv(cwd: &Path) -> Vec<String> {
    let mut args = vec![
        "app-server".into(),
        "-c".into(),
        "cli_auth_credentials_store=\"ephemeral\"".into(),
    ];
    for key in DISABLED {
        args.extend(["-c".into(), format!("features.{key}=false")]);
    }
    args.extend([
        "-c".into(),
        "notify=[]".into(),
        "-c".into(),
        "approval_policy=\"never\"".into(),
        "-c".into(),
        "web_search=\"disabled\"".into(),
        "-c".into(),
        format!(
            "projects={}",
            toml(&json!({cwd.to_string_lossy().as_ref():{"trust_level":"untrusted"}}))
        ),
    ]);
    args
}
impl RestrictedProfile {
    fn filesystem(&self) -> Value {
        let mut v = json!({":root":"deny",":minimal":"read",":tmpdir":"deny",":slash_tmp":"deny",self.cwd.to_string_lossy().as_ref():"read"});
        for p in &self.denied {
            v[p.to_string_lossy().as_ref()] = json!("deny");
        }
        v
    }
    fn profile(&self) -> Value {
        json!({"filesystem":self.filesystem(),"network":{"enabled":false}})
    }
    pub(crate) fn argv(&self, mcp: &Value) -> Vec<String> {
        let mut values = vec![
            ("approval_policy".to_string(), json!("never")),
            ("default_permissions".into(), json!(PROFILE_ID)),
            (format!("permissions.{PROFILE_ID}"), self.profile()),
            ("web_search".into(), json!("disabled")),
            ("mcp_servers".into(), mcp.clone()),
            ("cli_auth_credentials_store".into(), json!("ephemeral")),
            (
                "projects".into(),
                json!({self.cwd.to_string_lossy().as_ref():{"trust_level":"untrusted"}}),
            ),
            ("notify".into(), json!([])),
        ];
        values.extend(
            DISABLED
                .iter()
                .map(|k| (format!("features.{k}"), json!(false))),
        );
        let mut args = vec!["app-server".into()];
        for (k, v) in values {
            args.extend(["-c".into(), format!("{k}={}", toml(&v))]);
        }
        args
    }
    pub(crate) fn validate_config(&self, result: &Value, expected_mcp: &Value) -> bool {
        let Some(c) = result.get("config") else {
            return false;
        };
        if provider_fingerprint(c).as_deref() != Some(self.provider_fingerprint.as_str())
            || c["model_provider"].as_str() != Some(self.model_provider.as_str())
            || c["approval_policy"] != "never"
            || c["default_permissions"] != PROFILE_ID
            || c["web_search"] != "disabled"
            || c["cli_auth_credentials_store"] != "ephemeral"
        {
            return false;
        }
        if DISABLED.iter().any(|k| c["features"][*k] != false) {
            return false;
        }
        let p = &c["permissions"][PROFILE_ID];
        if !p["extends"].is_null()
            || !p["workspace_roots"].is_null()
            || p["network"]["enabled"] != false
        {
            return false;
        }
        let Some(fs) = p["filesystem"].as_object() else {
            return false;
        };
        let expected = self.filesystem();
        if fs
            .iter()
            .any(|(k, v)| !v.is_null() && expected.get(k) != Some(v))
            || expected
                .as_object()
                .unwrap()
                .iter()
                .any(|(k, v)| fs.get(k) != Some(v))
        {
            return false;
        }
        let Some(servers) = c["mcp_servers"].as_object() else {
            return false;
        };
        if servers
            .iter()
            .any(|(name, v)| name != "crew" && v["enabled"] != false)
        {
            return false;
        }
        let Some(crew) = servers.get("crew") else {
            return false;
        };
        let wanted = &expected_mcp["crew"];
        if crew["command"] != wanted["command"]
            || crew["args"] != wanted["args"]
            || crew["required"] != true
            || crew["enabled"] == false
            || !crew["url"].is_null()
        {
            return false;
        }
        for key in ["env", "env_vars", "http_headers", "env_http_headers"] {
            let v = &crew[key];
            if !v.is_null()
                && !v.as_object().is_some_and(|m| m.is_empty())
                && !v.as_array().is_some_and(|a| a.is_empty())
            {
                return false;
            }
        }
        for key in [
            "cwd",
            "http_headers_helper",
            "bearer_token_env_var",
            "oauth_resource",
            "oauth",
        ] {
            if !crew[key].is_null() {
                return false;
            }
        }
        if !crew["experimental_environment"].is_null()
            && crew["experimental_environment"] != "local"
        {
            return false;
        }
        if c.get("notify")
            .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
        {
            return false;
        }
        if c.get("hooks")
            .is_some_and(|v| !v.is_null() && v.as_object().is_none_or(|m| !m.is_empty()))
        {
            return false;
        }
        let Some(layers) = result["layers"].as_array() else {
            return false;
        };
        for layer in layers {
            if layer["disabledReason"].is_string() {
                continue;
            }
            match layer["name"]["type"].as_str() {
                Some("project") => return false,
                Some("user")
                    if layer["name"]["file"].as_str() != self.home.join("config.toml").to_str() =>
                {
                    return false;
                }
                Some(_) => {}
                None => return false,
            }
        }
        true
    }
    pub(crate) fn validate_open(&self, result: &Value) -> bool {
        result["modelProvider"].as_str() == Some(self.model_provider.as_str())
            && result["approvalPolicy"] == "never"
            && result["sandbox"]["type"] == "readOnly"
            && result["sandbox"]["networkAccess"] == false
            && result["activePermissionProfile"]["id"] == PROFILE_ID
            && result["cwd"].as_str() == self.cwd.to_str()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn auth_fixture(user: &str, account: &str, exp: i64, salt: &str) -> Value {
        let claims = json!({"exp":exp,"https://api.openai.com/auth":{"chatgpt_user_id":user,"chatgpt_account_id":account,"chatgpt_plan_type":"pro"}});
        let token = format!(
            "synthetic.{}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&claims).unwrap()),
            salt
        );
        json!({"authMethod":"chatgpt","requiresOpenaiAuth":true,"authToken":token})
    }
    #[test]
    fn external_access_is_opaque_and_account_user_bound() {
        let first =
            ExternalChatgptAuth::from_native(&auth_fixture("user-a", "account-a", 1000, "a"), 1)
                .unwrap();
        let rotated =
            ExternalChatgptAuth::from_native(&auth_fixture("user-a", "account-a", 1000, "b"), 1)
                .unwrap();
        let other_user =
            ExternalChatgptAuth::from_native(&auth_fixture("user-b", "account-a", 1000, "a"), 1)
                .unwrap();
        let other_account =
            ExternalChatgptAuth::from_native(&auth_fixture("user-a", "account-b", 1000, "a"), 1)
                .unwrap();
        assert!(first.same_identity(&rotated));
        assert!(first.changed_access(&rotated));
        assert!(!first.changed_access(&first));
        assert!(!first.same_identity(&other_user));
        assert!(!first.same_identity(&other_account));
        assert_eq!(format!("{first:?}"), "ExternalChatgptAuth([REDACTED])");
        assert!(first.login_params().get("refreshToken").is_none());
        assert!(first.refresh_result().get("idToken").is_none());
    }
    #[test]
    fn malformed_expired_or_other_auth_methods_are_not_exported() {
        let mut v = auth_fixture("user", "account", 1000, "a");
        assert_eq!(
            ExternalChatgptAuth::from_native(&v, 970),
            Err(AuthFailure::Expired)
        );
        v["authMethod"] = json!("apikey");
        assert_eq!(
            ExternalChatgptAuth::from_native(&v, 1),
            Err(AuthFailure::Unsupported)
        );
        v["authMethod"] = json!("chatgpt");
        v["authToken"] = json!("sensitive-but-malformed");
        assert_eq!(
            ExternalChatgptAuth::from_native(&v, 1),
            Err(AuthFailure::Unavailable)
        );
        let no_user = auth_fixture("", "account", 1000, "a");
        assert_eq!(
            ExternalChatgptAuth::from_native(&no_user, 1),
            Err(AuthFailure::Unavailable)
        );
    }
    pub(crate) fn fixture() -> (RestrictedProfile, Value, Value) {
        let p = RestrictedProfile {
            model_provider: "openai".into(),
            provider_fingerprint: provider_fingerprint(&json!({"model_provider":"openai"}))
                .unwrap(),
            home: "/runtime/case".into(),
            cwd: "/project".into(),
            denied: vec!["/runtime".into(), "/auth".into()],
            external_auth: Some(
                ExternalChatgptAuth::from_native(&auth_fixture("u", "a", 4102444800, "a"), 1)
                    .unwrap(),
            ),
        };
        let mcp = json!({"crew":{"command":"/helper","args":["mcp"],"required":true}});
        let mut config = json!({"model_provider":"openai","approval_policy":"never","default_permissions":PROFILE_ID,"web_search":"disabled","cli_auth_credentials_store":"ephemeral","features":{},"permissions":{PROFILE_ID:p.profile()},"mcp_servers":mcp,"notify":[],"hooks":{}});
        for key in DISABLED {
            config["features"][*key] = json!(false)
        }
        (p, config, mcp)
    }
    #[test]
    fn effective_configuration_requires_complete_profile_and_only_trusted_mcp() {
        let (p, c, mcp) = fixture();
        let good = json!({"config":c,"layers":[{"name":{"type":"sessionFlags"},"config":{}}]});
        assert!(p.validate_config(&good, &mcp));
        for path in [
            "/config/approval_policy",
            "/config/features/plugins",
            "/config/permissions/pcw_admin_case_v1/filesystem/:root",
            "/config/permissions/pcw_admin_case_v1/network/enabled",
            "/layers",
        ] {
            let mut bad = good.clone();
            *bad.pointer_mut(path).unwrap() = Value::Null;
            assert!(!p.validate_config(&bad, &mcp), "{path}");
        }
        let mut rerouted = good.clone();
        rerouted["config"]["model_providers"]["openai"] =
            json!({"base_url":"http://untrusted.invalid"});
        assert!(!p.validate_config(&rerouted, &mcp));
        let mut extra = good.clone();
        extra["config"]["mcp_servers"]["untrusted"] = json!({"command":"/foreign"});
        assert!(!p.validate_config(&extra, &mcp));
        let mut extra = good.clone();
        extra["config"]["mcp_servers"]["crew"]["env"] = json!({"INJECT":"value"});
        assert!(!p.validate_config(&extra, &mcp));
        let mut extra = good;
        extra["layers"] = json!([{"name":{"type":"project"}}]);
        assert!(!p.validate_config(&extra, &mcp));
    }
    #[test]
    fn private_home_never_copies_config_and_reuses_its_bound_state() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let work = temp.path().join("work");
        let runtime = temp.path().join("runtime");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&work).unwrap();
        create_private(&source.join("auth.json"),br#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic-only","refresh_token":"synthetic-only"}}"#).unwrap();
        fs::write(source.join("config.toml"), "synthetic secret config").unwrap();
        let mut env =
            BTreeMap::from([("CODEX_HOME".into(), source.to_string_lossy().into_owned())]);
        let auth =
            ExternalChatgptAuth::from_native(&auth_fixture("u", "a", 4102444800, "a"), 1).unwrap();
        let first = prepare(&runtime, "case-1", &work, &mut env, &auth).unwrap();
        assert!(!first.home.join("config.toml").exists());
        assert_eq!(
            fs::metadata(&first.home).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(!first.home.join("auth.json").exists());
        // A native Keychain-backed owner need not have an auth.json at all.
        // This is a filesystem contract test, not a claim of live Keychain use.
        fs::remove_file(source.join("auth.json")).unwrap();
        env.insert("CODEX_HOME".into(), source.to_string_lossy().into_owned());
        let without_file = prepare(&runtime, "case-no-file", &work, &mut env, &auth).unwrap();
        assert!(!without_file.home.join("auth.json").exists());
        env.insert("CODEX_HOME".into(), source.to_string_lossy().into_owned());
        assert_eq!(
            prepare(&runtime, "case-1", &work, &mut env, &auth).unwrap(),
            first
        );
        fs::write(first.home.join("config.toml"), "injected").unwrap();
        env.insert("CODEX_HOME".into(), source.to_string_lossy().into_owned());
        assert!(prepare(&runtime, "case-1", &work, &mut env, &auth).is_err());
    }
    #[test]
    fn routing_probe_cannot_load_persistent_auth_or_create_another_refresh_owner() {
        let argv = auth_probe_argv(Path::new("/project"));
        assert!(
            argv.windows(2)
                .any(|a| a[0] == "-c" && a[1] == "cli_auth_credentials_store=\"ephemeral\"")
        );
    }
    #[test]
    fn unexpected_open_policy_cannot_be_counted_as_enforced() {
        let (p, _, _) = fixture();
        let good = json!({"modelProvider":"openai","approvalPolicy":"never","sandbox":{"type":"readOnly","networkAccess":false},"activePermissionProfile":{"id":PROFILE_ID},"cwd":"/project"});
        assert!(p.validate_open(&good));
        for path in [
            "/approvalPolicy",
            "/sandbox/networkAccess",
            "/activePermissionProfile/id",
        ] {
            let mut bad = good.clone();
            *bad.pointer_mut(path).unwrap() = Value::Null;
            assert!(!p.validate_open(&bad));
        }
    }
    #[tokio::test]
    async fn executable_environment_is_rejected_before_even_source_auth_probe() {
        use crate::codex::driver::{CodexConfig, CodexSpec, CodexStart, start_codex};
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("must-not-run");
        let program = temp.path().join("native-probe");
        fs::write(
            &program,
            format!("#!/bin/sh\ntouch '{}'\nexit 17\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        for name in [
            "NODE_OPTIONS",
            "BASH_ENV",
            "DYLD_INSERT_LIBRARIES",
            "LD_PRELOAD",
            "CODEX_HOME",
        ] {
            let result = start_codex(
                CodexStart {
                    session_id: "synthetic-case".into(),
                    program: program.clone(),
                    child_path: "/usr/bin:/bin".into(),
                    ledger: std::sync::Arc::new(crate::resume::MemoryLedger::default()),
                    spec: CodexSpec {
                        native_policy: NativePolicy::AdminCaseRestricted,
                        workdir: temp.path().into(),
                        additional_env: BTreeMap::from([(
                            name.into(),
                            "synthetic-injection".into(),
                        )]),
                        ..Default::default()
                    },
                },
                CodexConfig::default(),
            )
            .await;
            assert!(matches!(result, Err(crate::driver::StartError::Files(_))));
            assert!(!marker.exists());
        }
    }
}
