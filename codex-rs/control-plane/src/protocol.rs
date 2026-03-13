use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchKind {
    Fresh,
    Resume,
    Fork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    InProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplySteerResult {
    Applied,
    RejectedNotEnabled,
    RejectedNotAuthorized,
    RejectedNoActiveTurn,
    RejectedStaleTurn,
    Duplicate,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplySteerRequest {
    pub expected_turn_id: String,
    pub text: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum Request {
    Status,
    CurrentSession,
    CurrentTurn,
    ApplySteer(ApplySteerRequest),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum Response {
    Status {
        payload: StatusPayload,
    },
    CurrentSession {
        payload: Option<CurrentSessionPayload>,
    },
    CurrentTurn {
        payload: Option<CurrentTurnPayload>,
    },
    ApplySteer {
        payload: ApplySteerPayload,
    },
    Error {
        payload: ErrorPayload,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusPayload {
    pub instance_id: String,
    pub socket_path: String,
    pub launch_kind: LaunchKind,
    pub feature_enabled: bool,
    pub consent_accepted: bool,
    pub steering_enabled: bool,
    pub session_known: bool,
    pub active_turn_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentSessionPayload {
    pub thread_id: String,
    pub thread_name: Option<String>,
    pub cwd: String,
    pub launch_kind: LaunchKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentTurnPayload {
    pub thread_id: String,
    pub turn_id: String,
    pub status: TurnStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplySteerPayload {
    pub outcome: ApplySteerResult,
    pub turn_id: Option<String>,
    pub actual_turn_id: Option<String>,
    pub message: Option<String>,
}

impl ApplySteerPayload {
    pub fn new(outcome: ApplySteerResult) -> Self {
        Self {
            outcome,
            turn_id: None,
            actual_turn_id: None,
            message: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub message: String,
}

pub fn socket_path(ipc_dir: &Path, instance_id: &str) -> PathBuf {
    let socket_id: String = instance_id.chars().take(12).collect();
    ipc_dir.join(format!("{socket_id}.sock"))
}
