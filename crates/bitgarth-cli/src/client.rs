use std::fmt;
use std::io::Read as _;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use reqwest::blocking::Response;
use reqwest::header::{AUTHORIZATION, HeaderValue, RETRY_AFTER};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use url::Url;
use zeroize::Zeroizing;

use crate::profiles::{ProfileError, SecretClientKey, canonicalize_origin};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PAIRING_BODY_LIMIT: usize = 64 * 1024;
pub(crate) const BALANCES_BODY_LIMIT: usize = 8 * 1024 * 1024;
const MINIMUM_POLL_DELAY: Duration = Duration::from_secs(5);
const VERIFIER_DOMAIN: &[u8] = b"bitgarth-client-key-verifier-v1\0";

pub(crate) struct ServerOrigin {
    url: Url,
    allow_insecure_http: bool,
}

impl ServerOrigin {
    pub(crate) fn parse(input: &str, allow_insecure_http: bool) -> Result<Self, ClientError> {
        let url = canonicalize_origin(input).map_err(ClientError::Profile)?;
        match (url.scheme(), allow_insecure_http) {
            ("https", false) | ("http", true) => Ok(Self {
                url,
                allow_insecure_http,
            }),
            ("http", false) => Err(ClientError::InvalidOrigin(
                "HTTP requires --allow-insecure-http for this exact origin",
            )),
            ("https", true) => Err(ClientError::InvalidOrigin(
                "--allow-insecure-http is only valid with an HTTP origin",
            )),
            _ => Err(ClientError::InvalidOrigin("unsupported server origin")),
        }
    }

    pub(crate) fn url(&self) -> &Url {
        &self.url
    }

    pub(crate) fn allows_insecure_http(&self) -> bool {
        self.allow_insecure_http
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url, ClientError> {
        let mut url = self.url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|()| ClientError::InvalidOrigin("server origin cannot be a base URL"))?;
            path.clear();
            path.extend(segments);
        }
        Ok(url)
    }
}

pub(crate) struct BitGarthClient {
    origin: ServerOrigin,
    client: reqwest::blocking::Client,
}

impl BitGarthClient {
    pub(crate) fn new(origin: ServerOrigin) -> Result<Self, ClientError> {
        Self::build(origin, None)
    }

    pub(crate) fn origin(&self) -> &Url {
        self.origin.url()
    }

    fn build(
        origin: ServerOrigin,
        test_root: Option<reqwest::Certificate>,
    ) -> Result<Self, ClientError> {
        let mut builder = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none());
        if origin.allows_insecure_http() {
            builder = builder.no_proxy();
        }
        if let Some(root) = test_root {
            builder = builder.add_root_certificate(root);
        }
        let client = builder
            .build()
            .map_err(|_| ClientError::Transport(origin.url().to_string()))?;
        Ok(Self { origin, client })
    }

    #[cfg(test)]
    fn with_test_root(origin: ServerOrigin, certificate_der: &[u8]) -> Result<Self, ClientError> {
        let certificate = reqwest::Certificate::from_der(certificate_der)
            .map_err(|_| ClientError::InvalidResponse("invalid test certificate"))?;
        Self::build(origin, Some(certificate))
    }

    pub(crate) fn start_pairing(
        &self,
        profile_name: &str,
        verifier: &str,
    ) -> Result<PairingStartResponse, ClientError> {
        let url = self.origin.endpoint(&["api", "v1", "pairings"])?;
        let request = PairingStartRequest {
            client_name: profile_name,
            key_verifier: verifier,
            permissions: ["balances_read"],
        };
        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .map_err(|_| ClientError::Transport(self.origin.url().to_string()))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(ClientError::PairingUnsupported(
                self.origin.url().to_string(),
            ));
        }
        decode_success(response, PAIRING_BODY_LIMIT, self.origin.url())
    }

    pub(crate) fn claim_pairing(
        &self,
        pairing_id: &str,
        client_key: &SecretClientKey,
    ) -> Result<PairingClaim, ClientError> {
        let url = self
            .origin
            .endpoint(&["api", "v1", "pairings", pairing_id, "claim"])?;
        let header = authorization_header(client_key)?;
        let response = self
            .client
            .post(url)
            .header(AUTHORIZATION, header)
            .send()
            .map_err(|_| ClientError::Transport(self.origin.url().to_string()))?;
        let retry_after = parse_retry_after(response.headers().get(RETRY_AFTER), Utc::now());
        let status = response.status();
        let claim: PairingClaimResponse =
            decode_success(response, PAIRING_BODY_LIMIT, self.origin.url())?;
        match (status, claim) {
            (StatusCode::ACCEPTED, PairingClaimResponse::Pending) => Ok(PairingClaim::Pending {
                retry_after: retry_after.max(MINIMUM_POLL_DELAY),
            }),
            (
                StatusCode::OK,
                PairingClaimResponse::Active {
                    remote_user_id,
                    permissions,
                },
            ) if !remote_user_id.is_empty() && permissions.as_slice() == ["balances_read"] => {
                Ok(PairingClaim::Active { remote_user_id })
            }
            _ => Err(ClientError::InvalidResponse(
                "incompatible pairing claim response",
            )),
        }
    }

    pub(crate) fn wallet_balances(
        &self,
        client_key: &SecretClientKey,
    ) -> Result<crate::output::WalletBalances, ClientError> {
        let url = self.origin.endpoint(&["api", "v1", "wallet-balances"])?;
        let response = self
            .client
            .get(url)
            .header(AUTHORIZATION, authorization_header(client_key)?)
            .send()
            .map_err(|_| ClientError::Transport(self.origin.url().to_string()))?;
        match response.status() {
            StatusCode::UNAUTHORIZED => return Err(ClientError::BalancesUnauthorized),
            StatusCode::FORBIDDEN => return Err(ClientError::BalancesForbidden),
            _ => {}
        }
        let balances: crate::output::WalletBalances =
            decode_success(response, BALANCES_BODY_LIMIT, self.origin.url())?;
        balances
            .validate()
            .map_err(|_| ClientError::InvalidResponse("incompatible wallet balance response"))?;
        Ok(balances)
    }
}

fn authorization_header(client_key: &SecretClientKey) -> Result<HeaderValue, ClientError> {
    let mut authorization = Zeroizing::new(String::with_capacity(7 + client_key.as_str().len()));
    authorization.push_str("Bearer ");
    authorization.push_str(client_key.as_str());
    let mut header = HeaderValue::from_str(authorization.as_str())
        .map_err(|_| ClientError::InvalidResponse("invalid Client Key header"))?;
    header.set_sensitive(true);
    Ok(header)
}

pub(crate) fn generate_client_key() -> Result<Zeroizing<[u8; 32]>, ClientError> {
    let mut key = Zeroizing::new([0_u8; 32]);
    getrandom::fill(key.as_mut()).map_err(|_| ClientError::RandomnessUnavailable)?;
    Ok(key)
}

pub(crate) fn key_verifier(key: &[u8; 32]) -> Zeroizing<String> {
    let mut hasher = Sha256::new();
    hasher.update(VERIFIER_DOMAIN);
    hasher.update(key);
    let digest = Zeroizing::new(<[u8; 32]>::from(hasher.finalize()));
    Zeroizing::new(URL_SAFE_NO_PAD.encode(digest.as_slice()))
}

#[derive(Serialize)]
struct PairingStartRequest<'a> {
    client_name: &'a str,
    key_verifier: &'a str,
    permissions: [&'a str; 1],
}

#[derive(Deserialize)]
pub(crate) struct PairingStartResponse {
    pub(crate) pairing_id: String,
    pub(crate) code: String,
    pub(crate) approval_url: String,
    pub(crate) expires_at: String,
}

impl PairingStartResponse {
    pub(crate) fn validate(&self, expected: &Url) -> Result<(DateTime<Utc>, Url), ClientError> {
        let pairing_id = URL_SAFE_NO_PAD
            .decode(&self.pairing_id)
            .map_err(|_| ClientError::InvalidResponse("invalid pairing ID"))?;
        if pairing_id.len() != 32 || URL_SAFE_NO_PAD.encode(pairing_id) != self.pairing_id {
            return Err(ClientError::InvalidResponse("invalid pairing ID"));
        }
        let valid_code = self.code.len() == 9
            && self.code.as_bytes().get(4) == Some(&b'-')
            && self.code.chars().enumerate().all(|(index, character)| {
                index == 4 || "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(character)
            });
        if !valid_code {
            return Err(ClientError::InvalidResponse("invalid pairing code"));
        }
        let expires_at = DateTime::parse_from_rfc3339(&self.expires_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|_| ClientError::InvalidResponse("invalid pairing expiry"))?;

        let approval = Url::parse(&self.approval_url)
            .map_err(|_| ClientError::InvalidResponse("invalid pairing approval URL"))?;
        if !approval.username().is_empty() || approval.password().is_some() {
            return Err(ClientError::InvalidResponse("invalid pairing approval URL"));
        }
        if approval.scheme() != expected.scheme()
            || approval.host_str() != expected.host_str()
            || approval.port_or_known_default() != expected.port_or_known_default()
        {
            return Err(ClientError::ApprovalOriginMismatch {
                expected: expected.origin().ascii_serialization(),
                returned: approval.origin().ascii_serialization(),
            });
        }
        let matching_codes = approval
            .query_pairs()
            .filter(|(name, value)| name == "code" && value.as_ref() == self.code.as_str())
            .count();
        if approval.path() != "/pair" || matching_codes != 1 {
            return Err(ClientError::InvalidResponse("invalid pairing approval URL"));
        }
        Ok((expires_at, approval))
    }
}

pub(crate) enum PairingClaim {
    Pending { retry_after: Duration },
    Active { remote_user_id: String },
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum PairingClaimResponse {
    Pending,
    Active {
        remote_user_id: String,
        permissions: Vec<String>,
    },
}

#[derive(Deserialize)]
pub(crate) struct PublicErrorEnvelope {
    code: String,
    message: String,
}

impl PublicErrorEnvelope {
    fn is_safe(&self) -> bool {
        !self.code.is_empty()
            && self.code.len() <= 64
            && self
                .code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            && !self.message.is_empty()
            && self.message.len() <= 1024
            && !self.message.chars().any(char::is_control)
    }
}

fn decode_success<T: DeserializeOwned>(
    response: Response,
    limit: usize,
    origin: &Url,
) -> Result<T, ClientError> {
    let status = response.status();
    let bytes = read_limited(response, limit, origin)?;
    if !status.is_success() {
        let public = serde_json::from_slice::<PublicErrorEnvelope>(&bytes)
            .ok()
            .filter(PublicErrorEnvelope::is_safe);
        return Err(ClientError::Http {
            origin: origin.to_string(),
            status,
            public,
        });
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ClientError::InvalidResponse("invalid server response"))
}

fn read_limited(
    mut response: Response,
    limit: usize,
    origin: &Url,
) -> Result<Vec<u8>, ClientError> {
    let limit = limit.min(BALANCES_BODY_LIMIT);
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(ClientError::ResponseTooLarge(origin.to_string()));
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ClientError::Transport(origin.to_string()))?;
    if bytes.len() > limit {
        return Err(ClientError::ResponseTooLarge(origin.to_string()));
    }
    Ok(bytes)
}

fn parse_retry_after(value: Option<&HeaderValue>, now: DateTime<Utc>) -> Duration {
    let Some(value) = value.and_then(|value| value.to_str().ok()) else {
        return MINIMUM_POLL_DELAY;
    };
    if let Ok(seconds) = value.parse::<u64>() {
        return Duration::from_secs(seconds);
    }
    DateTime::parse_from_rfc2822(value)
        .ok()
        .and_then(|date| {
            let seconds = (date.with_timezone(&Utc) - now).num_seconds();
            u64::try_from(seconds).ok()
        })
        .map(Duration::from_secs)
        .unwrap_or(MINIMUM_POLL_DELAY)
}

pub(crate) enum ClientError {
    Profile(ProfileError),
    InvalidOrigin(&'static str),
    Transport(String),
    Http {
        origin: String,
        status: StatusCode,
        public: Option<PublicErrorEnvelope>,
    },
    PairingUnsupported(String),
    ResponseTooLarge(String),
    InvalidResponse(&'static str),
    ApprovalOriginMismatch {
        expected: String,
        returned: String,
    },
    RandomnessUnavailable,
    PairingExpired,
    BalancesUnauthorized,
    BalancesForbidden,
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Profile(error) => write!(formatter, "{error}"),
            Self::InvalidOrigin(message) | Self::InvalidResponse(message) => {
                formatter.write_str(message)
            }
            Self::ApprovalOriginMismatch { expected, returned } => write!(
                formatter,
                "Pairing stopped: you connected to {expected}, but the server asked you to approve pairing at {returned}. BitGarth will not open an approval link at a different address or security level. If you entered the correct BitGarth URL, this is a server configuration problem."
            ),
            Self::Transport(origin) => write!(formatter, "request to {origin} failed"),
            Self::Http {
                origin,
                status,
                public,
            } => {
                write!(formatter, "request to {origin} failed with HTTP {status}")?;
                if let Some(public) = public {
                    write!(formatter, ": {}: {}", public.code, public.message)?;
                }
                Ok(())
            }
            Self::PairingUnsupported(origin) => {
                write!(formatter, "server {origin} does not support CLI pairing")
            }
            Self::ResponseTooLarge(origin) => {
                write!(formatter, "response from {origin} exceeded the size limit")
            }
            Self::RandomnessUnavailable => formatter.write_str("OS randomness is unavailable"),
            Self::PairingExpired => formatter.write_str("pairing request expired before approval"),
            Self::BalancesUnauthorized => formatter.write_str(
                "this profile's Client Key was rejected; pair the profile again",
            ),
            Self::BalancesForbidden => formatter.write_str(
                "this profile lacks balances_read permission or was revoked; review paired clients or pair again",
            ),
        }
    }
}

impl fmt::Debug for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for ClientError {}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use serde::Deserialize;

    const TEST_CERT_DER: &str = "MIIBvTCCAWOgAwIBAgIUdwz+Rwj2YxWVW/aPPEFcYEW/ncUwCgYIKoZIzj0EAwIwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDgwMTA4MzEzNloXDTM2MDcyOTA4MzEzNlowFDESMBAGA1UEAwwJbG9jYWxob3N0MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEs+ZIKAZ3vPBs73cpIPUt6UO9G7z3Q+tJqP4ogo3OsLsGdekzxEYcbF0TzMq7Pp66n/Zb8OrS+FyMbHOih/F/HqOBkjCBjzAdBgNVHQ4EFgQUyJaPysVY9AKC3VXpJYapRH3VSwwwHwYDVR0jBBgwFoAUyJaPysVY9AKC3VXpJYapRH3VSwwwDAYDVR0TAQH/BAIwADAOBgNVHQ8BAf8EBAMCB4AwEwYDVR0lBAwwCgYIKwYBBQUHAwEwGgYDVR0RBBMwEYIJbG9jYWxob3N0hwR/AAABMAoGCCqGSM49BAMCA0gAMEUCIHelae9oYQDnZzyMNNerOZPn1RVPUZ7FdxAqQoLg9dreAiEA9PhTsISw4hUZXNyQ+6AW0wzDJWxWzBIcsRm2hFfTJN0=";
    const TEST_KEY_DER: &str = "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgX0id8N+52fuXJD6O/Iv/9SGzvt7HDgSXiIHc37XTNV6hRANCAASz5kgoBne88Gzvdykg9S3pQ70bvPdD60mo/iiCjc6wuwZ16TPERhxsXRPMyrs+nrqf9lvw6tL4XIxsc6KH8X8e";

    use super::{
        BitGarthClient, ClientError, PairingClaim, ServerOrigin, key_verifier, parse_retry_after,
    };
    use crate::profiles::SecretClientKey;

    #[test]
    fn canonicalizes_scheme_host_and_effective_port() {
        let origin = ServerOrigin::parse("https://EXAMPLE.com:443", false);
        assert!(origin.is_ok());
        assert_eq!(
            origin.ok().map(|value| value.url().to_string()),
            Some("https://example.com/".to_owned())
        );
    }

    #[test]
    fn canonicalizes_browser_urls_and_rejects_invalid_origins_or_implicit_http_trust() {
        for input in [
            "https://example.com/path",
            "https://example.com/?query=yes",
            "https://example.com/#fragment",
        ] {
            assert_eq!(
                ServerOrigin::parse(input, false)
                    .ok()
                    .map(|origin| origin.url().to_string()),
                Some("https://example.com/".to_owned())
            );
        }

        for input in [
            "https://user@example.com/",
            "https:///missing-host",
            "ftp://example.com/",
        ] {
            assert!(
                ServerOrigin::parse(input, false).is_err(),
                "accepted {input}"
            );
        }
        assert!(ServerOrigin::parse("http://127.0.0.1/", false).is_err());
        assert!(ServerOrigin::parse("http://192.168.1.4/", false).is_err());
        assert!(ServerOrigin::parse("http://100.64.0.1/", false).is_err());
        assert!(ServerOrigin::parse("http://127.0.0.1/", true).is_ok());
        assert!(ServerOrigin::parse("https://example.com/", true).is_err());
    }

    #[test]
    fn key_and_verifier_match_shared_fixture() {
        #[derive(Deserialize)]
        struct Fixture {
            client_key: String,
            key_verifier: String,
        }

        let fixture: Result<Fixture, _> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/client_api/client-key.json"
        ));
        assert!(fixture.is_ok());
        let Ok(fixture) = fixture else {
            return;
        };
        let bytes = std::array::from_fn(|index| index as u8);
        let key = SecretClientKey::from_bytes(&bytes);
        assert_eq!(key.as_str(), fixture.client_key);
        assert_eq!(key_verifier(&bytes).as_str(), fixture.key_verifier);
    }

    #[test]
    fn start_and_claim_match_shared_fixtures() {
        let start_fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/client_api/pairing-start.json"
        ))
        .unwrap_or(serde_json::Value::Null);
        let claim_fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/client_api/pairing-claim.json"
        ))
        .unwrap_or(serde_json::Value::Null);
        assert_ne!(start_fixture, serde_json::Value::Null);
        assert_ne!(claim_fixture, serde_json::Value::Null);

        let request = super::PairingStartRequest {
            client_name: "business",
            key_verifier: "geFJOqW5wsUi7LidS2OuPuOOylI2EV4fFLr5Rmaos_k",
            permissions: ["balances_read"],
        };
        assert_eq!(
            serde_json::to_value(request).ok(),
            Some(start_fixture["request"].clone())
        );
        let start: Result<super::PairingStartResponse, _> =
            serde_json::from_value(start_fixture["response"].clone());
        assert!(start.is_ok());
        let pending: Result<super::PairingClaimResponse, _> =
            serde_json::from_value(claim_fixture["pending_response"].clone());
        let active: Result<super::PairingClaimResponse, _> =
            serde_json::from_value(claim_fixture["active_response"].clone());
        assert!(matches!(pending, Ok(super::PairingClaimResponse::Pending)));
        assert!(matches!(
            active,
            Ok(super::PairingClaimResponse::Active { .. })
        ));
    }

    #[test]
    fn redirect_is_returned_without_following() {
        let (origin, requests, handle) = http_server([
            "HTTP/1.1 302 Found\r\nLocation: /second\r\nContent-Length: 0\r\n\r\n".to_owned(),
        ]);
        let origin = ServerOrigin::parse(&origin, true);
        assert!(origin.is_ok());
        let Ok(origin) = origin else { return };
        let client = BitGarthClient::new(origin);
        assert!(client.is_ok());
        let Ok(client) = client else { return };
        let result = client.start_pairing("profile", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        assert!(matches!(result, Err(ClientError::Http { status, .. }) if status.as_u16() == 302));
        assert!(handle.join().is_ok());
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn claim_attaches_key_and_honors_longer_retry_after() {
        let body = r#"{"status":"pending"}"#;
        let response = format!(
            "HTTP/1.1 202 Accepted\r\nRetry-After: 17\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{body}",
            body.len()
        );
        let (origin, _, handle) = http_server([response]);
        let origin = ServerOrigin::parse(&origin, true);
        assert!(origin.is_ok());
        let Ok(origin) = origin else { return };
        let client = BitGarthClient::new(origin);
        assert!(client.is_ok());
        let Ok(client) = client else { return };
        let key = SecretClientKey::from_bytes(&[0; 32]);
        let claim = client.claim_pairing("AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE", &key);
        assert!(
            matches!(claim, Ok(PairingClaim::Pending { retry_after }) if retry_after.as_secs() == 17)
        );
        assert!(handle.join().is_ok());
    }

    #[test]
    fn retry_after_never_accelerates_polling() {
        let short = reqwest::header::HeaderValue::from_static("1");
        assert_eq!(
            parse_retry_after(Some(&short), chrono::Utc::now()).max(super::MINIMUM_POLL_DELAY),
            std::time::Duration::from_secs(5)
        );

        let now = chrono::DateTime::parse_from_rfc3339("2026-08-01T08:00:00Z")
            .map(|value| value.with_timezone(&chrono::Utc));
        assert!(now.is_ok());
        let Ok(now) = now else { return };
        let later = reqwest::header::HeaderValue::from_static("Sat, 01 Aug 2026 08:00:19 GMT");
        assert_eq!(parse_retry_after(Some(&later), now).as_secs(), 19);
    }

    #[test]
    fn approval_url_must_keep_the_configured_origin() {
        let response = super::PairingStartResponse {
            pairing_id: "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE".to_owned(),
            code: "1111-1111".to_owned(),
            approval_url: "https://other.example:8443/pair?code=1111-1111".to_owned(),
            expires_at: "2030-01-01T00:00:00Z".to_owned(),
        };
        let expected = url::Url::parse("https://example.com/");
        assert!(expected.is_ok());
        let Ok(expected) = expected else { return };
        let result = response.validate(&expected);
        assert!(result.is_err());
        let Err(error) = result else { return };
        assert_eq!(
            error.to_string(),
            "Pairing stopped: you connected to https://example.com, but the server asked you to approve pairing at https://other.example:8443. BitGarth will not open an approval link at a different address or security level. If you entered the correct BitGarth URL, this is a server configuration problem."
        );
    }

    #[test]
    fn start_404_has_pairing_specific_guidance() {
        let (origin, _, server) =
            http_server(["HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_owned()]);
        let parsed = ServerOrigin::parse(&origin, true);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else { return };
        let client = BitGarthClient::new(parsed);
        assert!(client.is_ok());
        let Ok(client) = client else { return };
        let result = client.start_pairing("profile", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        assert!(matches!(result, Err(ClientError::PairingUnsupported(_))));
        assert!(server.join().is_ok());
    }

    #[test]
    fn test_root_constructor_rejects_invalid_certificate() {
        let origin = ServerOrigin::parse("https://localhost/", false);
        assert!(origin.is_ok());
        let Ok(origin) = origin else { return };
        assert!(BitGarthClient::with_test_root(origin, b"not a certificate").is_err());
    }

    #[test]
    fn trusted_https_works_and_default_roots_reject_test_certificate() {
        let body = r#"{"pairing_id":"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE","code":"1111-1111","approval_url":"https://localhost/pair?code=1111-1111","expires_at":"2030-01-01T00:00:00Z"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{body}",
            body.len()
        );
        let (origin, certificate, trusted_server) = tls_server(response.clone());
        let parsed = ServerOrigin::parse(&origin, false);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else { return };
        let client = BitGarthClient::with_test_root(parsed, &certificate);
        assert!(client.is_ok());
        let Ok(client) = client else { return };
        let started =
            client.start_pairing("profile", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        assert!(
            started.is_ok(),
            "{}",
            started
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default()
        );
        assert!(trusted_server.join().is_ok());

        let (origin, _, untrusted_server) = tls_server(response);
        let parsed = ServerOrigin::parse(&origin, false);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else { return };
        let client = BitGarthClient::new(parsed);
        assert!(client.is_ok());
        let Ok(client) = client else { return };
        let failure =
            client.start_pairing("profile", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        assert!(matches!(failure, Err(ClientError::Transport(_))));
        assert!(untrusted_server.join().is_ok());
    }

    #[test]
    fn https_to_http_redirect_is_not_followed() {
        let response =
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/unsafe\r\nContent-Length: 0\r\n\r\n"
                .to_owned();
        let (origin, certificate, server) = tls_server(response);
        let parsed = ServerOrigin::parse(&origin, false);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else { return };
        let client = BitGarthClient::with_test_root(parsed, &certificate);
        assert!(client.is_ok());
        let Ok(client) = client else { return };
        let result = client.start_pairing("profile", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        assert!(
            matches!(&result, Err(ClientError::Http { status, .. }) if status.as_u16() == 302),
            "{}",
            result
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default()
        );
        assert!(server.join().is_ok());
    }

    #[test]
    fn diagnostics_exclude_credentials_verifier_and_raw_body() {
        let body = "not-json Authorization raw-private-body";
        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let (origin, _, server) = http_server([response]);
        let parsed = ServerOrigin::parse(&origin, true);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else { return };
        let client = BitGarthClient::new(parsed);
        assert!(client.is_ok());
        let Ok(client) = client else { return };
        let key = SecretClientKey::from_bytes(&[0; 32]);
        let result = client.claim_pairing("pairing", &key);
        assert!(result.is_err());
        let Err(error) = result else { return };
        let diagnostic = error.to_string();
        assert!(!diagnostic.contains(key.as_str()));
        assert!(!diagnostic.contains("Authorization"));
        assert!(!diagnostic.contains("raw-private-body"));
        assert!(server.join().is_ok());
    }

    #[test]
    fn wallet_balances_consumes_shared_fixture() {
        let body = include_str!("../../../tests/fixtures/client_api/wallet-balances.json");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{body}",
            body.len()
        );
        let (origin, _, server) = http_server([response]);
        let parsed = ServerOrigin::parse(&origin, true);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else { return };
        let client = BitGarthClient::new(parsed);
        assert!(client.is_ok());
        let Ok(client) = client else { return };
        let key = SecretClientKey::from_bytes(&[0; 32]);
        let balances = client.wallet_balances(&key);
        assert!(balances.is_ok());
        let Ok(balances) = balances else { return };
        assert!(balances.render().is_ok());
        assert!(server.join().is_ok());
    }

    #[test]
    fn wallet_auth_errors_ignore_response_content() {
        for (status, expected) in [
            (401, "Client Key was rejected"),
            (403, "lacks balances_read permission"),
        ] {
            let body = "Authorization secret-private-response";
            let response = format!(
                "HTTP/1.1 {status} Error\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let (origin, _, server) = http_server([response]);
            let parsed = ServerOrigin::parse(&origin, true);
            assert!(parsed.is_ok());
            let Ok(parsed) = parsed else { continue };
            let client = BitGarthClient::new(parsed);
            assert!(client.is_ok());
            let Ok(client) = client else { continue };
            let key = SecretClientKey::from_bytes(&[0; 32]);
            let result = client.wallet_balances(&key);
            assert!(result.is_err());
            let Err(error) = result else { continue };
            let diagnostic = error.to_string();
            assert!(diagnostic.contains(expected));
            assert!(!diagnostic.contains(key.as_str()));
            assert!(!diagnostic.contains("Authorization"));
            assert!(!diagnostic.contains("secret-private-response"));
            assert!(server.join().is_ok());
        }
    }

    #[test]
    fn wallet_response_is_bounded_to_eight_mebibytes() {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            super::BALANCES_BODY_LIMIT + 1
        );
        let (origin, _, server) = http_server([response]);
        let parsed = ServerOrigin::parse(&origin, true);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else { return };
        let client = BitGarthClient::new(parsed);
        assert!(client.is_ok());
        let Ok(client) = client else { return };
        let key = SecretClientKey::from_bytes(&[0; 32]);
        assert!(matches!(
            client.wallet_balances(&key),
            Err(ClientError::ResponseTooLarge(_))
        ));
        assert!(server.join().is_ok());
    }

    fn http_server<const N: usize>(
        responses: [String; N],
    ) -> (
        String,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0");
        assert!(listener.is_ok());
        let Ok(listener) = listener else {
            std::process::abort();
        };
        let address = listener.local_addr();
        assert!(address.is_ok());
        let address = match address {
            Ok(address) => address,
            Err(_) => std::process::abort(),
        };
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let request_count = std::sync::Arc::clone(&requests);
        let handle = thread::spawn(move || {
            for response in responses {
                let accepted = listener.accept();
                assert!(accepted.is_ok());
                let Ok((mut stream, _)) = accepted else {
                    return;
                };
                let mut request = [0_u8; 4096];
                assert!(stream.read(&mut request).is_ok());
                request_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert!(stream.write_all(response.as_bytes()).is_ok());
            }
        });
        (format!("http://{address}/"), requests, handle)
    }

    fn tls_server(response: String) -> (String, Vec<u8>, thread::JoinHandle<()>) {
        use base64::Engine as _;
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

        let certificate = base64::engine::general_purpose::STANDARD.decode(TEST_CERT_DER);
        let key = base64::engine::general_purpose::STANDARD.decode(TEST_KEY_DER);
        assert!(certificate.is_ok());
        assert!(key.is_ok());
        let Ok(certificate) = certificate else {
            std::process::abort();
        };
        let Ok(key) = key else {
            std::process::abort();
        };
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(certificate.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
            );
        assert!(config.is_ok());
        let Ok(config) = config else {
            std::process::abort();
        };
        let listener = TcpListener::bind("127.0.0.1:0");
        assert!(listener.is_ok());
        let Ok(listener) = listener else {
            std::process::abort();
        };
        let address = listener.local_addr();
        assert!(address.is_ok());
        let address = match address {
            Ok(address) => address,
            Err(_) => std::process::abort(),
        };
        let handle = thread::spawn(move || {
            let accepted = listener.accept();
            assert!(accepted.is_ok());
            let Ok((stream, _)) = accepted else { return };
            let connection = rustls::ServerConnection::new(std::sync::Arc::new(config));
            assert!(connection.is_ok());
            let Ok(connection) = connection else { return };
            let mut tls = rustls::StreamOwned::new(connection, stream);
            let mut request = [0_u8; 4096];
            if tls.read(&mut request).is_ok() {
                let _ = tls.write_all(response.as_bytes());
                let _ = tls.flush();
            }
        });
        (
            format!("https://localhost:{}/", address.port()),
            certificate,
            handle,
        )
    }
}
