//! Shared command context: the selected login, API calls, and CLI errors.

use std::fmt;

use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::args::GlobalArgs;
use crate::client::{BitGarthClient, ClientError, ServerOrigin};
use crate::profiles::{Profile, ProfileError, ProfileStore};

pub(crate) struct Context {
    global: GlobalArgs,
}

impl Context {
    pub(crate) fn new(global: GlobalArgs) -> Self {
        Self { global }
    }

    pub(crate) fn json(&self) -> bool {
        self.global.json
    }

    pub(crate) fn profile_name(&self) -> Option<&str> {
        self.global.profile.as_deref()
    }

    /// Connect using the selected saved login.
    pub(crate) fn session(&self) -> Result<Session, CliError> {
        let store = ProfileStore::load()?;
        let profile = store
            .into_selected(self.profile_name())
            .map_err(|error| CliError::NotLoggedIn(error.to_string()))?;
        Session::connect(profile)
    }

    /// Print a JSON value to stdout, pretty and newline-terminated.
    pub(crate) fn print_json(&self, value: &Value) -> Result<(), CliError> {
        let text = serde_json::to_string_pretty(value)
            .map_err(|_| CliError::Failed("could not encode JSON output".to_owned()))?;
        println!("{text}");
        Ok(())
    }
}

pub(crate) struct Session {
    pub(crate) profile: Profile,
    client: BitGarthClient,
}

impl Session {
    pub(crate) fn connect(profile: Profile) -> Result<Self, CliError> {
        let origin = ServerOrigin::parse(
            profile.canonical_origin().as_str(),
            profile.allows_insecure_http(),
        )?;
        let client = BitGarthClient::new(origin)?;
        Ok(Self { profile, client })
    }

    pub(crate) fn call(
        &self,
        method: Method,
        path: &[&str],
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value, CliError> {
        self.client
            .request(self.profile.client_key(), method, path, query, body)
            .map_err(|error| self.explain(error))
    }

    pub(crate) fn get(&self, path: &[&str], query: &[(&str, String)]) -> Result<Value, CliError> {
        self.call(Method::GET, path, query, None)
    }

    pub(crate) fn send(
        &self,
        method: Method,
        path: &[&str],
        body: &Value,
    ) -> Result<Value, CliError> {
        self.call(method, path, &[], Some(body))
    }

    pub(crate) fn delete(&self, path: &[&str]) -> Result<Value, CliError> {
        self.call(Method::DELETE, path, &[], None)
    }

    pub(crate) fn origin(&self) -> &str {
        self.profile.canonical_origin().as_str()
    }

    fn explain(&self, error: ClientError) -> CliError {
        let name = self.profile.name().to_owned();
        match error.status() {
            Some(401) => CliError::Unauthorized { profile: name },
            Some(403) => CliError::Denied {
                message: error.to_string(),
                read_only: self.profile.permission() != "full",
                profile: name,
                origin: self.origin().to_owned(),
            },
            Some(_) => CliError::Api {
                message: error.to_string(),
                details: error.field_messages(),
                status: error.status(),
            },
            None => CliError::Failed(error.to_string()),
        }
    }
}

/// Decode a JSON value into a typed view, explaining version skew on failure.
pub(crate) fn decode<T: DeserializeOwned>(value: Value) -> Result<T, CliError> {
    serde_json::from_value(value).map_err(|error| {
        CliError::Failed(format!(
            "unexpected response from the server ({error}); the server and CLI versions may differ"
        ))
    })
}

#[derive(Debug)]
pub(crate) enum CliError {
    /// Invalid arguments or input; exit code 2.
    Usage(String),
    /// No usable saved login; exit code 4.
    NotLoggedIn(String),
    /// The server rejected the Client Key; exit code 4.
    Unauthorized {
        profile: String,
    },
    /// The login lacks permission for this action; exit code 4.
    Denied {
        message: String,
        read_only: bool,
        profile: String,
        origin: String,
    },
    /// The server refused the request; exit code 1.
    Api {
        message: String,
        details: Vec<String>,
        status: Option<u16>,
    },
    /// Anything else that went wrong; exit code 1.
    Failed(String),
    Cancelled,
}

impl CliError {
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            Self::NotLoggedIn(_) | Self::Unauthorized { .. } | Self::Denied { .. } => 4,
            Self::Api { .. } | Self::Failed(_) | Self::Cancelled => 1,
        }
    }

    pub(crate) fn details(&self) -> &[String] {
        match self {
            Self::Api { details, .. } => details,
            _ => &[],
        }
    }

    pub(crate) fn hint(&self) -> Option<String> {
        match self {
            Self::NotLoggedIn(_) => {
                Some("Pair this computer with `bitgarth auth login <URL>`; see `bitgarth auth login --help`.".to_owned())
            }
            Self::Unauthorized { profile } => Some(format!(
                "Log in again with `bitgarth auth logout --local-only --profile {profile}` followed by `bitgarth auth login`."
            )),
            Self::Denied {
                read_only: true,
                profile,
                origin,
                ..
            } => {
                let insecure = if origin.starts_with("http://") {
                    " --allow-insecure-http"
                } else {
                    ""
                };
                Some(format!(
                    "Profile {profile} is read-only. Run `bitgarth auth logout --profile {profile}`, then `bitgarth auth login {origin} --profile {profile}{insecure}` to pair with full access."
                ))
            }
            Self::Api {
                status: Some(404), ..
            } => Some("List what exists with `bitgarth wallet list` or `bitgarth account list`.".to_owned()),
            _ => None,
        }
    }

    pub(crate) fn is_status(&self, code: u16) -> bool {
        matches!(self, Self::Api { status: Some(status), .. } if *status == code)
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::NotLoggedIn(message) | Self::Failed(message) => {
                formatter.write_str(message)
            }
            Self::Unauthorized { profile } => write!(
                formatter,
                "The server no longer accepts profile {profile}: it was revoked, expired, or its account was deleted."
            ),
            Self::Denied { message, .. } | Self::Api { message, .. } => {
                formatter.write_str(message)
            }
            Self::Cancelled => formatter.write_str("Cancelled."),
        }
    }
}

impl From<ClientError> for CliError {
    fn from(error: ClientError) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<ProfileError> for CliError {
    fn from(error: ProfileError) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(format!("terminal I/O failed: {error}"))
    }
}
