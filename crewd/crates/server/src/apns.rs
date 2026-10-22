//! Closed native APNs transport. No caller-selected URL, proxy, redirects or retries.
//! A successful response is provider acceptance only, never business/device success.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair},
};
use serde::{Deserialize, Serialize};
use std::{sync::Mutex, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Environment {
    Sandbox,
    Production,
}
impl Environment {
    fn host(self) -> &'static str {
        match self {
            Self::Sandbox => "api.sandbox.push.apple.com",
            Self::Production => "api.push.apple.com",
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Result {
    Accepted,
    Rejected(String),
    Unknown,
}
impl Result {
    pub(crate) fn invalidates_token(&self) -> bool {
        matches!(self, Self::Rejected(reason) if matches!(reason.as_str(), "BadDeviceToken"|"DeviceTokenNotForTopic"|"Unregistered"))
    }
}
// Deliberately neither Debug nor Serialize: no key/JWT/caller URL in diagnostics.
pub(crate) struct Sender {
    client: reqwest::Client,
    key: EcdsaKeyPair,
    kid: String,
    team: String,
    topic: String,
    environment: Environment,
    jwt: Mutex<Option<(i64, String)>>,
    #[cfg(test)]
    test_endpoint: Option<String>,
}
fn id(s: &str) -> bool {
    s.len() == 10
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}
fn safe_topic(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
}
pub(crate) fn valid_token(s: &str) -> bool {
    s.len() >= 2
        && s.len() <= 512
        && s.len() % 2 == 0
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .http2_prior_knowledge()
        .https_only(true)
        .tls_version_min(reqwest::tls::Version::TLS_1_2)
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .connection_verbose(false)
}
impl Sender {
    pub(crate) fn new(
        kid: String,
        team: String,
        topic: String,
        environment: Environment,
        pkcs8: &[u8],
    ) -> std::result::Result<Self, ()> {
        if !id(&kid) || !id(&team) || !safe_topic(&topic) || pkcs8.len() > 8192 {
            return Err(());
        }
        let key = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8,
            &SystemRandom::new(),
        )
        .map_err(|_| ())?;
        Ok(Self {
            client: client_builder().build().map_err(|_| ())?,
            key,
            kid,
            team,
            topic,
            environment,
            jwt: Mutex::new(None),
            #[cfg(test)]
            test_endpoint: None,
        })
    }
    #[cfg(test)]
    pub(crate) fn local_test_sender(
        tls: rustls::ClientConfig,
        endpoint: String,
        pkcs8: &[u8],
    ) -> Self {
        let mut sender = Self::new(
            "ABCDEFGHIJ".into(),
            "1234567890".into(),
            "com.example.app".into(),
            Environment::Sandbox,
            pkcs8,
        )
        .unwrap();
        sender.client = client_builder()
            .tls_backend_preconfigured(tls)
            .build()
            .unwrap();
        sender.test_endpoint = Some(endpoint);
        sender
    }
    fn authorization(&self, now: i64) -> std::result::Result<String, ()> {
        if now < 0 {
            return Err(());
        }
        let mut cache = self.jwt.lock().map_err(|_| ())?;
        if let Some((iat, value)) = cache.as_ref() {
            if now < *iat {
                return Err(());
            }
            if now - *iat < 1800 {
                return Ok(value.clone());
            }
        }
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({"alg":"ES256","kid":self.kid}))
                .map_err(|_| ())?,
        );
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({"iss":self.team,"iat":now})).map_err(|_| ())?,
        );
        let unsigned = format!("{header}.{claims}");
        let signature = self
            .key
            .sign(&SystemRandom::new(), unsigned.as_bytes())
            .map_err(|_| ())?;
        let value = format!("{unsigned}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
        *cache = Some((now, value.clone()));
        Ok(value)
    }
    pub(crate) async fn send(
        &self,
        token: &str,
        apns_id: &str,
        payload: &[u8],
        now: i64,
        expires_at: i64,
    ) -> Result {
        if !valid_token(token)
            || !crewd_proto::valid_remote_uuid(apns_id)
            || payload.len() > 4096
            || expires_at <= now
        {
            return Result::Unknown;
        }
        let Ok(jwt) = self.authorization(now) else {
            return Result::Unknown;
        };
        let Ok(mut auth) = reqwest::header::HeaderValue::from_str(&format!("bearer {jwt}")) else {
            return Result::Unknown;
        };
        auth.set_sensitive(true);
        let endpoint = format!("https://{}/3/device/{token}", self.environment.host());
        #[cfg(test)]
        let endpoint = self
            .test_endpoint
            .as_ref()
            .map(|e| format!("{e}/3/device/{token}"))
            .unwrap_or(endpoint);
        let request = self
            .client
            .post(endpoint)
            .header(reqwest::header::AUTHORIZATION, auth)
            .header("apns-id", apns_id)
            .header("apns-topic", &self.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", "5")
            .header("apns-expiration", expires_at.to_string())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload.to_vec());
        let Ok(mut response) = request.send().await else {
            return Result::Unknown;
        };
        if response.version() != reqwest::Version::HTTP_2
            || response
                .headers()
                .get("apns-id")
                .and_then(|v| v.to_str().ok())
                != Some(apns_id)
        {
            return Result::Unknown;
        }
        let status = response.status().as_u16();
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len() + chunk.len() > 4096 {
                        return Result::Unknown;
                    }
                    body.extend_from_slice(&chunk)
                }
                Ok(None) => break,
                Err(_) => return Result::Unknown,
            }
        }
        if status == 200 {
            return if body.is_empty() {
                Result::Accepted
            } else {
                Result::Unknown
            };
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&body) else {
            return Result::Unknown;
        };
        let Some(reason) = value.get("reason").and_then(|v| v.as_str()) else {
            return Result::Unknown;
        };
        if !(400..600).contains(&status)
            || !matches!(
                reason,
                "BadDeviceToken"
                    | "DeviceTokenNotForTopic"
                    | "Unregistered"
                    | "BadTopic"
                    | "BadMessageId"
                    | "PayloadTooLarge"
                    | "InvalidProviderToken"
                    | "ExpiredProviderToken"
                    | "MissingProviderToken"
                    | "Forbidden"
                    | "TooManyRequests"
                    | "InternalServerError"
                    | "ServiceUnavailable"
                    | "Shutdown"
                    | "BadPriority"
                    | "BadExpirationDate"
                    | "DuplicateHeaders"
                    | "MissingTopic"
                    | "TopicDisallowed"
                    | "BadCollapseId"
                    | "TooManyProviderTokenUpdates"
            )
        {
            return Result::Unknown;
        }
        Result::Rejected(reason.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, KeyPair, UnparsedPublicKey};
    #[test]
    fn synthetic_es256_is_fixed_width_cached_and_never_follows_clock_backwards() {
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .unwrap();
        let sender = Sender::new(
            "ABCDEFGHIJ".into(),
            "1234567890".into(),
            "com.pendingname.pendingcrew".into(),
            Environment::Sandbox,
            pkcs8.as_ref(),
        )
        .unwrap();
        let token = sender.authorization(10000).unwrap();
        assert_eq!(sender.authorization(11000).unwrap(), token);
        let parts = token.split('.').collect::<Vec<_>>();
        let sig = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        assert_eq!(sig.len(), 64);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, sender.key.public_key())
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .unwrap();
        assert!(sender.authorization(9999).is_err());
        assert_ne!(sender.authorization(11800).unwrap(), token);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap())
                .unwrap()["iat"],
            10000
        );
        assert!(
            Sender::new(
                "bad".into(),
                "1234567890".into(),
                "com.test".into(),
                Environment::Sandbox,
                pkcs8.as_ref()
            )
            .is_err()
        );
    }
    #[test]
    fn token_and_invalidation_are_bounded_and_closed() {
        assert!(valid_token("01ab"));
        assert!(!valid_token("ABCDEF"));
        assert!(!valid_token("ab/path"));
        assert!(!valid_token(&"ab".repeat(257)));
        assert!(Result::Rejected("Unregistered".into()).invalidates_token());
        assert!(!Result::Unknown.invalidates_token());
        assert!(!Result::Rejected("TooManyRequests".into()).invalidates_token());
    }
    async fn local_case(
        status: u16,
        body: &'static [u8],
        wrong_id: bool,
        drop_reply: bool,
    ) -> Result {
        use std::{
            collections::BTreeSet,
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
        };
        let server = crewd_remote::CertificateIdentity::generate().unwrap();
        let client = crewd_remote::CertificateIdentity::generate().unwrap();
        let pins = Arc::new(crewd_remote::LiveClientPins::default());
        pins.replace(BTreeSet::from([client.pin().to_string()]))
            .unwrap();
        let mut tls = crewd_remote::server_configuration(&server, pins).unwrap();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        let mut client_tls = crewd_remote::client_configuration(&client, server.pin()).unwrap();
        client_tls.alpn_protocols = vec![b"h2".to_vec()];
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let observed = count.clone();
        let key =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .unwrap();
        let mut sender = Sender::new(
            "ABCDEFGHIJ".into(),
            "1234567890".into(),
            "com.pendingname.pendingcrew".into(),
            Environment::Sandbox,
            key.as_ref(),
        )
        .unwrap();
        let public = sender.key.public_key().as_ref().to_vec();
        let task = tokio::spawn(async move {
            tokio::time::timeout(Duration::from_secs(12), async move {
                eprintln!("synthetic APNs h2 phase: accept TCP");
            let (socket, _) = listener.accept().await.unwrap();
            eprintln!("synthetic APNs h2 phase: TLS handshake");
            let tls = tokio_rustls::TlsAcceptor::from(Arc::new(tls))
                .accept(socket)
                .await
                .unwrap();
            assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
            eprintln!("synthetic APNs h2 phase: h2 handshake");
            let mut conn = h2::server::handshake(tls).await.unwrap();
            eprintln!("synthetic APNs h2 phase: request headers");
            let (request, mut response) = conn.accept().await.unwrap().unwrap();
            observed.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.method(), "POST");
            assert_eq!(request.uri().path(), "/3/device/01ab");
            assert_eq!(
                request.headers()["apns-topic"],
                "com.pendingname.pendingcrew"
            );
            assert_eq!(request.headers()["apns-push-type"], "alert");
            assert_eq!(request.headers()["apns-expiration"], "96400");
            let auth = request.headers()["authorization"]
                .to_str()
                .unwrap()
                .strip_prefix("bearer ")
                .unwrap();
            let parts = auth.split('.').collect::<Vec<_>>();
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public)
                .verify(
                    format!("{}.{}", parts[0], parts[1]).as_bytes(),
                    &URL_SAFE_NO_PAD.decode(parts[2]).unwrap(),
                )
                .unwrap();
            let requested = request.headers()["apns-id"].clone();
            let mut stream = request.into_body();
            let mut payload = Vec::new();
            eprintln!("synthetic APNs h2 phase: read DATA while driving Connection");
            loop {
                // h2.0.4.20 requires Connection polling to advance socket I/O;
                // RecvStream::data alone cannot fetch a later DATA frame.
                let next = tokio::select! {
                    data = stream.data() => data,
                    _ = conn.accept() => panic!("synthetic APNs h2 connection ended or received another stream before the request body completed"),
                };
                let Some(data) = next else { break };
                let data = data.unwrap();
                payload.extend_from_slice(&data);
                stream.flow_control().release_capacity(data.len()).unwrap();
            }
            eprintln!("synthetic APNs h2 phase: complete request body");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&payload).unwrap()["context"]["action"],
                "open_context"
            );
            if drop_reply {
                return;
            }
            let id = if wrong_id {
                "00000000-0000-0000-0000-000000000000".parse().unwrap()
            } else {
                requested
            };
            let r = http::Response::builder()
                .status(status)
                .header("apns-id", id)
                .body(())
                .unwrap();
            let mut send = response.send_response(r, body.is_empty()).unwrap();
            if !body.is_empty() {
                send.send_data(body.to_vec().into(), true).unwrap();
            }
            let _ = tokio::time::timeout(Duration::from_millis(500), conn.accept()).await;
            // No second request is allowed by the client retry policy.
            assert_eq!(observed.load(Ordering::SeqCst), 1);
            }).await.expect("synthetic APNs h2 server exceeded its 12s phase deadline");
        });
        sender.client = client_builder()
            .tls_backend_preconfigured(client_tls)
            .build()
            .unwrap();
        sender.test_endpoint = Some(format!("https://localhost:{}", addr.port()));
        let payload =
            br#"{"aps":{"alert":"PendingCrew update"},"context":{"action":"open_context"}}"#;
        let result = sender
            .send(
                "01ab",
                "01234567-89ab-4cde-8fab-0123456789ab",
                payload,
                10000,
                96400,
            )
            .await;
        task.await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        result
    }
    #[tokio::test]
    async fn actual_local_tls_h2_uses_es256_exact_headers_and_acceptance_only() {
        assert_eq!(local_case(200, b"", false, false).await, Result::Accepted);
    }
    #[tokio::test]
    async fn actual_local_h2_rejections_unknown_and_lost_reply_never_retry() {
        assert_eq!(
            local_case(410, br#"{"reason":"Unregistered"}"#, false, false).await,
            Result::Rejected("Unregistered".into())
        );
        assert_eq!(
            local_case(429, br#"{"reason":"TooManyRequests"}"#, false, false).await,
            Result::Rejected("TooManyRequests".into())
        );
        assert_eq!(local_case(200, b"", true, false).await, Result::Unknown);
        assert_eq!(
            local_case(500, b"invalid", false, false).await,
            Result::Unknown
        );
        assert_eq!(local_case(200, b"", false, true).await, Result::Unknown);
    }
}
