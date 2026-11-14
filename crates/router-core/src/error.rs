use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ApiErrorKind {
    Token,
    NoRights,
    Busy,
    BadParam,
    NotSupported,
    BadCredentials,
    AlreadyLoggedIn,
    TooManyAttempts,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum RouterError {
    #[error("router unreachable")]
    Unreachable,
    #[error("request timed out")]
    Timeout,
    #[error("router API error {code} ({kind:?})")]
    Api {
        code: u32,
        #[serde(rename = "errorKind")]
        #[ts(rename = "errorKind")]
        kind: ApiErrorKind,
    },
    /// The router has no session for us. `code` is the router's own error code when it said so
    /// (100003, 125002 or 125003), and `None` when the app concluded it without an answer.
    #[error("session lost{}", router_code_suffix(*code))]
    #[serde(rename_all = "camelCase")]
    SessionLost {
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        code: Option<u32>,
    },
    #[error("session taken over by another login")]
    SessionTakenOver,
    #[error("bad credentials")]
    BadCredentials,
    #[error("locked out for {wait_s} s")]
    #[serde(rename_all = "camelCase")]
    LockedOut { wait_s: u32 },
    /// The router refused the login with a code the app doesn't recognise. Like
    /// `BadCredentials`, it is never retried automatically (lockout guard).
    #[error("login rejected by the router (code {code})")]
    LoginRejected { code: u32 },
    #[error("login algorithm mismatch")]
    LoginAlgorithmMismatch,
    /// Stopped before sending a login proof: the user has to say this is their router.
    #[error("router not recognised")]
    #[serde(rename_all = "camelCase")]
    UnrecognizedRouter { first_time: bool },
    #[error("token exhausted")]
    TokenExhausted,
    #[error("parse error: {message}")]
    Parse { message: String },
    #[error("unsupported by this router")]
    Unsupported,
    #[error("validation failed: {message}")]
    Validation { message: String },
}

impl RouterError {
    /// Whether this is `SessionLost`, with or without a router code.
    pub fn is_session_lost(&self) -> bool {
        matches!(self, RouterError::SessionLost { .. })
    }

    /// The router's own error code behind this error, when there is one.
    pub fn router_code(&self) -> Option<u32> {
        match self {
            RouterError::Api { code, .. } | RouterError::LoginRejected { code } => Some(*code),
            RouterError::SessionLost { code } => *code,
            _ => None,
        }
    }
}

fn router_code_suffix(code: Option<u32>) -> String {
    code.map(|c| format!(" (router code {c})"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_lost_keeps_the_router_code() {
        let coded = RouterError::SessionLost { code: Some(100003) };
        assert_eq!(coded.to_string(), "session lost (router code 100003)");
        assert_eq!(
            serde_json::to_value(&coded).unwrap(),
            serde_json::json!({"kind": "sessionLost", "code": 100003})
        );
        assert_eq!(coded.router_code(), Some(100003));
        assert!(coded.is_session_lost());

        let plain = RouterError::SessionLost { code: None };
        assert_eq!(plain.to_string(), "session lost");
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            serde_json::json!({"kind": "sessionLost"})
        );
        // Lines written before the code was kept still read.
        let old: RouterError = serde_json::from_str(r#"{"kind":"sessionLost"}"#).unwrap();
        assert_eq!(old, plain);
        assert!(!RouterError::SessionTakenOver.is_session_lost());
    }
}
