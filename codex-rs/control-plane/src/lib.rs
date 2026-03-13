mod protocol;
mod server;

use async_trait::async_trait;
use protocol::ApplySteerPayload;
pub use protocol::ApplySteerRequest;
pub use protocol::ApplySteerResult;
pub use protocol::CurrentSessionPayload;
pub use protocol::CurrentTurnPayload;
pub use protocol::ErrorPayload;
pub use protocol::LaunchKind;
pub use protocol::Request;
pub use protocol::Response;
pub use protocol::StatusPayload;
pub use protocol::TurnStatus;
use server::LocalIpcServer;
use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tracing::info;
use tracing::warn;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ControlPlaneConfig {
    pub feature_enabled: bool,
    pub consent_accepted: bool,
    pub steering_enabled: bool,
    pub ipc_dir: PathBuf,
    pub launch_kind: LaunchKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub thread_id: String,
    pub thread_name: Option<String>,
    pub cwd: PathBuf,
    pub launch_kind: LaunchKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSnapshot {
    pub turn_id: String,
    pub status: TurnStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPlaneSnapshot {
    pub instance_id: String,
    pub socket_path: PathBuf,
    pub feature_enabled: bool,
    pub consent_accepted: bool,
    pub steering_enabled: bool,
    pub launch_kind: LaunchKind,
    pub session: Option<SessionSnapshot>,
    pub active_turn: Option<TurnSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishedTurnStatus {
    Completed,
    Aborted,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteerError {
    NoActiveTurn,
    ExpectedTurnMismatch { actual_turn_id: String },
    Error(String),
}

#[async_trait]
pub trait SteerDelegate: Send + Sync {
    async fn apply_steer(
        &self,
        thread_id: &str,
        expected_turn_id: &str,
        text: String,
    ) -> Result<String, SteerError>;
}

#[derive(Debug)]
struct SessionState {
    thread_id: String,
    thread_name: Option<String>,
    cwd: PathBuf,
}

#[derive(Debug)]
struct ActiveTurnState {
    turn_id: String,
}

#[derive(Debug)]
struct State {
    feature_enabled: bool,
    consent_accepted: bool,
    steering_enabled: bool,
    launch_kind: LaunchKind,
    session: Option<SessionState>,
    active_turn: Option<ActiveTurnState>,
    seen_idempotency_keys: HashSet<(String, String)>,
}

impl State {
    fn status_payload(&self, instance_id: &str, socket_path: &Path) -> StatusPayload {
        StatusPayload {
            instance_id: instance_id.to_string(),
            socket_path: socket_path.display().to_string(),
            launch_kind: self.launch_kind,
            feature_enabled: self.feature_enabled,
            consent_accepted: self.consent_accepted,
            steering_enabled: self.steering_enabled,
            session_known: self.session.is_some(),
            active_turn_present: self.active_turn.is_some(),
        }
    }

    fn current_session_payload(&self) -> Option<CurrentSessionPayload> {
        self.session.as_ref().map(|session| CurrentSessionPayload {
            thread_id: session.thread_id.clone(),
            thread_name: session.thread_name.clone(),
            cwd: session.cwd.display().to_string(),
            launch_kind: self.launch_kind,
        })
    }

    fn current_turn_payload(&self) -> Option<CurrentTurnPayload> {
        self.session
            .as_ref()
            .zip(self.active_turn.as_ref())
            .map(|(session, turn)| CurrentTurnPayload {
                thread_id: session.thread_id.clone(),
                turn_id: turn.turn_id.clone(),
                status: TurnStatus::InProgress,
            })
    }
}

struct Shared {
    instance_id: String,
    socket_path: PathBuf,
    state: Mutex<State>,
    delegate: Arc<dyn SteerDelegate>,
}

impl Shared {
    async fn handle_request(&self, request: Request) -> Response {
        match request {
            Request::Status => {
                let payload = {
                    let state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.status_payload(&self.instance_id, &self.socket_path)
                };
                Response::Status { payload }
            }
            Request::CurrentSession => {
                let payload = {
                    let state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.current_session_payload()
                };
                Response::CurrentSession { payload }
            }
            Request::CurrentTurn => {
                let payload = {
                    let state = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.current_turn_payload()
                };
                Response::CurrentTurn { payload }
            }
            Request::ApplySteer(request) => self.handle_apply_steer(request).await,
        }
    }

    async fn handle_apply_steer(&self, request: ApplySteerRequest) -> Response {
        let metadata_keys = request.metadata.as_ref().and_then(|metadata| {
            metadata.as_object().map(|object| {
                let mut keys: Vec<String> = object.keys().cloned().collect();
                keys.sort();
                keys
            })
        });
        let reservation = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.feature_enabled {
                warn!(
                    instance_id = %self.instance_id,
                    metadata_keys = ?metadata_keys,
                    "control-plane steer request rejected because the feature is disabled"
                );
                return Response::ApplySteer {
                    payload: ApplySteerPayload::new(ApplySteerResult::RejectedNotEnabled),
                };
            }
            if !state.consent_accepted || !state.steering_enabled {
                warn!(
                    instance_id = %self.instance_id,
                    metadata_keys = ?metadata_keys,
                    "control-plane steer request rejected because steering is not authorized locally"
                );
                return Response::ApplySteer {
                    payload: ApplySteerPayload::new(ApplySteerResult::RejectedNotAuthorized),
                };
            }
            let Some(session) = state.session.as_ref() else {
                warn!(
                    instance_id = %self.instance_id,
                    metadata_keys = ?metadata_keys,
                    "control-plane steer request rejected because no session is registered"
                );
                return Response::ApplySteer {
                    payload: ApplySteerPayload::new(ApplySteerResult::RejectedNoActiveTurn),
                };
            };
            let thread_id = session.thread_id.clone();
            let Some(active_turn) = state.active_turn.as_ref() else {
                warn!(
                    instance_id = %self.instance_id,
                    thread_id,
                    metadata_keys = ?metadata_keys,
                    "control-plane steer request rejected because no active turn is present"
                );
                return Response::ApplySteer {
                    payload: ApplySteerPayload::new(ApplySteerResult::RejectedNoActiveTurn),
                };
            };
            let active_turn_id = active_turn.turn_id.clone();
            if request.expected_turn_id != active_turn_id {
                warn!(
                    instance_id = %self.instance_id,
                    thread_id,
                    expected_turn_id = request.expected_turn_id,
                    actual_turn_id = active_turn_id,
                    metadata_keys = ?metadata_keys,
                    "control-plane steer request rejected because the expected turn was stale"
                );
                return Response::ApplySteer {
                    payload: ApplySteerPayload {
                        outcome: ApplySteerResult::RejectedStaleTurn,
                        turn_id: None,
                        actual_turn_id: Some(active_turn_id),
                        message: None,
                    },
                };
            }
            let reservation = (
                request.expected_turn_id.clone(),
                request.idempotency_key.clone(),
            );
            if !state.seen_idempotency_keys.insert(reservation.clone()) {
                info!(
                    instance_id = %self.instance_id,
                    thread_id,
                    turn_id = request.expected_turn_id,
                    idempotency_key = request.idempotency_key,
                    metadata_keys = ?metadata_keys,
                    "control-plane steer request ignored as a duplicate"
                );
                return Response::ApplySteer {
                    payload: ApplySteerPayload {
                        outcome: ApplySteerResult::Duplicate,
                        turn_id: Some(request.expected_turn_id),
                        actual_turn_id: None,
                        message: None,
                    },
                };
            }
            if request.text.is_empty() {
                state.seen_idempotency_keys.remove(&reservation);
                warn!(
                    instance_id = %self.instance_id,
                    thread_id,
                    turn_id = request.expected_turn_id,
                    idempotency_key = request.idempotency_key,
                    metadata_keys = ?metadata_keys,
                    "control-plane steer request rejected because text was empty"
                );
                return Response::ApplySteer {
                    payload: ApplySteerPayload {
                        outcome: ApplySteerResult::Error,
                        turn_id: Some(request.expected_turn_id),
                        actual_turn_id: None,
                        message: Some("text must not be empty".to_string()),
                    },
                };
            }
            (thread_id, active_turn_id, reservation, request.text)
        };

        let (thread_id, active_turn_id, idempotency_reservation, text) = reservation;
        match self
            .delegate
            .apply_steer(&thread_id, &active_turn_id, text)
            .await
        {
            Ok(turn_id) => {
                info!(
                    instance_id = %self.instance_id,
                    thread_id,
                    turn_id,
                    expected_turn_id = active_turn_id,
                    idempotency_key = idempotency_reservation.1,
                    metadata_keys = ?metadata_keys,
                    "control-plane steer request applied"
                );
                Response::ApplySteer {
                    payload: ApplySteerPayload {
                        outcome: ApplySteerResult::Applied,
                        turn_id: Some(turn_id),
                        actual_turn_id: None,
                        message: None,
                    },
                }
            }
            Err(SteerError::NoActiveTurn) => {
                self.remove_idempotency_reservation(&idempotency_reservation);
                warn!(
                    instance_id = %self.instance_id,
                    thread_id,
                    expected_turn_id = active_turn_id,
                    idempotency_key = idempotency_reservation.1,
                    metadata_keys = ?metadata_keys,
                    "control-plane steer request rejected because no active turn remained"
                );
                Response::ApplySteer {
                    payload: ApplySteerPayload::new(ApplySteerResult::RejectedNoActiveTurn),
                }
            }
            Err(SteerError::ExpectedTurnMismatch { actual_turn_id }) => {
                self.remove_idempotency_reservation(&idempotency_reservation);
                warn!(
                    instance_id = %self.instance_id,
                    thread_id,
                    expected_turn_id = active_turn_id,
                    actual_turn_id,
                    idempotency_key = idempotency_reservation.1,
                    metadata_keys = ?metadata_keys,
                    "control-plane steer request rejected because the active turn changed"
                );
                Response::ApplySteer {
                    payload: ApplySteerPayload {
                        outcome: ApplySteerResult::RejectedStaleTurn,
                        turn_id: None,
                        actual_turn_id: Some(actual_turn_id),
                        message: None,
                    },
                }
            }
            Err(SteerError::Error(message)) => {
                self.remove_idempotency_reservation(&idempotency_reservation);
                warn!(
                    instance_id = %self.instance_id,
                    thread_id,
                    expected_turn_id = active_turn_id,
                    idempotency_key = idempotency_reservation.1,
                    metadata_keys = ?metadata_keys,
                    "control-plane steer request failed"
                );
                Response::ApplySteer {
                    payload: ApplySteerPayload {
                        outcome: ApplySteerResult::Error,
                        turn_id: Some(active_turn_id),
                        actual_turn_id: None,
                        message: Some(message),
                    },
                }
            }
        }
    }

    fn remove_idempotency_reservation(&self, reservation: &(String, String)) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.seen_idempotency_keys.remove(reservation);
    }
}

pub struct LocalControlPlane {
    shared: Arc<Shared>,
    _server: LocalIpcServer,
}

impl LocalControlPlane {
    pub fn start(config: ControlPlaneConfig, delegate: Arc<dyn SteerDelegate>) -> io::Result<Self> {
        let instance_id = Uuid::new_v4().to_string();
        let socket_path = protocol::socket_path(&config.ipc_dir, &instance_id);
        let shared = Arc::new(Shared {
            instance_id,
            socket_path: socket_path.clone(),
            state: Mutex::new(State {
                feature_enabled: config.feature_enabled,
                consent_accepted: config.consent_accepted,
                steering_enabled: config.steering_enabled,
                launch_kind: config.launch_kind,
                session: None,
                active_turn: None,
                seen_idempotency_keys: HashSet::new(),
            }),
            delegate,
        });
        let server = LocalIpcServer::start(socket_path, Arc::clone(&shared))?;
        info!(
            instance_id = %shared.instance_id,
            socket_path = %shared.socket_path.display(),
            feature_enabled = config.feature_enabled,
            consent_accepted = config.consent_accepted,
            steering_enabled = config.steering_enabled,
            launch_kind = ?config.launch_kind,
            "control-plane IPC server started"
        );
        Ok(Self {
            shared,
            _server: server,
        })
    }

    pub fn instance_id(&self) -> &str {
        &self.shared.instance_id
    }

    pub async fn apply_steer(&self, request: ApplySteerRequest) -> ApplySteerPayload {
        match self
            .shared
            .handle_request(Request::ApplySteer(request))
            .await
        {
            Response::ApplySteer { payload } => payload,
            Response::Error { payload } => ApplySteerPayload {
                outcome: ApplySteerResult::Error,
                turn_id: None,
                actual_turn_id: None,
                message: Some(payload.message),
            },
            _ => ApplySteerPayload {
                outcome: ApplySteerResult::Error,
                turn_id: None,
                actual_turn_id: None,
                message: Some("unexpected control-plane response".to_string()),
            },
        }
    }

    pub fn current_turn(&self) -> Option<CurrentTurnPayload> {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.current_turn_payload()
    }

    pub fn socket_path(&self) -> &Path {
        &self.shared.socket_path
    }

    pub fn snapshot(&self) -> ControlPlaneSnapshot {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ControlPlaneSnapshot {
            instance_id: self.shared.instance_id.clone(),
            socket_path: self.shared.socket_path.clone(),
            feature_enabled: state.feature_enabled,
            consent_accepted: state.consent_accepted,
            steering_enabled: state.steering_enabled,
            launch_kind: state.launch_kind,
            session: state
                .current_session_payload()
                .map(|session| SessionSnapshot {
                    thread_id: session.thread_id,
                    thread_name: session.thread_name,
                    cwd: PathBuf::from(session.cwd),
                    launch_kind: session.launch_kind,
                }),
            active_turn: state.current_turn_payload().map(|turn| TurnSnapshot {
                turn_id: turn.turn_id,
                status: turn.status,
            }),
        }
    }

    pub fn register_session(&self, thread_id: String, thread_name: Option<String>, cwd: PathBuf) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.session = Some(SessionState {
            thread_id: thread_id.clone(),
            thread_name: thread_name.clone(),
            cwd: cwd.clone(),
        });
        state.active_turn = None;
        state.seen_idempotency_keys.clear();
        info!(
            instance_id = %self.shared.instance_id,
            thread_id,
            thread_name = ?thread_name,
            cwd = %cwd.display(),
            launch_kind = ?state.launch_kind,
            "control-plane session registration changed"
        );
    }

    pub fn clear_session(&self, reason: &str) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_thread_id = state
            .session
            .as_ref()
            .map(|session| session.thread_id.clone());
        let previous_turn_id = state
            .active_turn
            .as_ref()
            .map(|active_turn| active_turn.turn_id.clone());
        state.session = None;
        state.active_turn = None;
        state.seen_idempotency_keys.clear();
        info!(
            instance_id = %self.shared.instance_id,
            previous_thread_id = ?previous_thread_id,
            previous_turn_id = ?previous_turn_id,
            reason,
            "control-plane session registration changed"
        );
    }

    pub fn note_turn_started(&self, turn_id: &str) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active_turn = Some(ActiveTurnState {
            turn_id: turn_id.to_string(),
        });
        state.seen_idempotency_keys.clear();
        info!(
            instance_id = %self.shared.instance_id,
            turn_id,
            "control-plane active turn changed"
        );
    }

    pub fn note_turn_finished(&self, turn_id: Option<&str>, status: FinishedTurnStatus) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let should_clear = match (&state.active_turn, turn_id) {
            (_, None) => true,
            (Some(active_turn), Some(turn_id)) => active_turn.turn_id == turn_id,
            (None, Some(_)) => false,
        };
        if should_clear {
            let cleared_turn_id = state
                .active_turn
                .take()
                .map(|active_turn| active_turn.turn_id);
            state.seen_idempotency_keys.clear();
            info!(
                instance_id = %self.shared.instance_id,
                turn_id = ?cleared_turn_id,
                finished_status = ?status,
                "control-plane active turn changed"
            );
        }
    }
}

#[cfg(test)]
impl LocalControlPlane {
    async fn handle_request_for_test(&self, request: Request) -> Response {
        self.shared.handle_request(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::fs;
    use tempfile::TempDir;

    struct StubDelegate {
        result: Mutex<Result<String, SteerError>>,
    }

    impl Default for StubDelegate {
        fn default() -> Self {
            Self {
                result: Mutex::new(Ok("turn-1".to_string())),
            }
        }
    }

    #[async_trait]
    impl SteerDelegate for StubDelegate {
        async fn apply_steer(
            &self,
            _thread_id: &str,
            _expected_turn_id: &str,
            _text: String,
        ) -> Result<String, SteerError> {
            self.result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    fn config(temp_dir: &TempDir) -> ControlPlaneConfig {
        ControlPlaneConfig {
            feature_enabled: true,
            consent_accepted: true,
            steering_enabled: true,
            ipc_dir: temp_dir.path().join("instances"),
            launch_kind: LaunchKind::Fresh,
        }
    }

    fn runtime(temp_dir: &TempDir) -> LocalControlPlane {
        LocalControlPlane::start(config(temp_dir), Arc::new(StubDelegate::default()))
            .expect("control plane should start")
    }

    #[tokio::test]
    async fn status_reports_runtime_metadata() {
        let temp_dir = TempDir::new().expect("tempdir");
        let runtime = runtime(&temp_dir);

        let response = runtime.handle_request_for_test(Request::Status).await;
        let Response::Status { payload } = response else {
            panic!("expected status response");
        };
        assert!(payload.feature_enabled);
        assert!(payload.consent_accepted);
        assert!(payload.steering_enabled);
        assert!(!payload.session_known);
        assert!(!payload.active_turn_present);
        assert!(payload.socket_path.ends_with(".sock"));
    }

    #[tokio::test]
    async fn current_session_and_turn_follow_lifecycle_updates() {
        let temp_dir = TempDir::new().expect("tempdir");
        let runtime = runtime(&temp_dir);

        runtime.register_session(
            "thread-1".to_string(),
            Some("hello".to_string()),
            temp_dir.path().to_path_buf(),
        );
        runtime.note_turn_started("turn-1");

        let session = runtime
            .handle_request_for_test(Request::CurrentSession)
            .await;
        let Response::CurrentSession { payload } = session else {
            panic!("expected current-session response");
        };
        assert_eq!(
            payload,
            Some(CurrentSessionPayload {
                thread_id: "thread-1".to_string(),
                thread_name: Some("hello".to_string()),
                cwd: temp_dir.path().display().to_string(),
                launch_kind: LaunchKind::Fresh,
            })
        );

        let turn = runtime.handle_request_for_test(Request::CurrentTurn).await;
        let Response::CurrentTurn { payload } = turn else {
            panic!("expected current-turn response");
        };
        assert_eq!(
            payload,
            Some(CurrentTurnPayload {
                thread_id: "thread-1".to_string(),
                turn_id: "turn-1".to_string(),
                status: TurnStatus::InProgress,
            })
        );

        runtime.note_turn_finished(Some("turn-1"), FinishedTurnStatus::Completed);
        runtime.clear_session("test");

        let turn = runtime.handle_request_for_test(Request::CurrentTurn).await;
        let Response::CurrentTurn { payload } = turn else {
            panic!("expected current-turn response");
        };
        assert_eq!(payload, None);
    }

    #[tokio::test]
    async fn apply_steer_rejects_not_enabled() {
        let temp_dir = TempDir::new().expect("tempdir");
        let runtime = LocalControlPlane::start(
            ControlPlaneConfig {
                feature_enabled: false,
                ..config(&temp_dir)
            },
            Arc::new(StubDelegate::default()),
        )
        .expect("control plane should start");

        let response = runtime
            .handle_request_for_test(Request::ApplySteer(ApplySteerRequest {
                expected_turn_id: "turn-1".to_string(),
                text: "hello".to_string(),
                idempotency_key: "abc".to_string(),
                metadata: None,
            }))
            .await;
        let Response::ApplySteer { payload } = response else {
            panic!("expected apply-steer response");
        };
        assert_eq!(payload.outcome, ApplySteerResult::RejectedNotEnabled);
    }

    #[tokio::test]
    async fn apply_steer_rejects_not_authorized() {
        let temp_dir = TempDir::new().expect("tempdir");
        let runtime = LocalControlPlane::start(
            ControlPlaneConfig {
                consent_accepted: false,
                ..config(&temp_dir)
            },
            Arc::new(StubDelegate::default()),
        )
        .expect("control plane should start");

        let response = runtime
            .handle_request_for_test(Request::ApplySteer(ApplySteerRequest {
                expected_turn_id: "turn-1".to_string(),
                text: "hello".to_string(),
                idempotency_key: "abc".to_string(),
                metadata: None,
            }))
            .await;
        let Response::ApplySteer { payload } = response else {
            panic!("expected apply-steer response");
        };
        assert_eq!(payload.outcome, ApplySteerResult::RejectedNotAuthorized);
    }

    #[tokio::test]
    async fn apply_steer_rejects_when_no_active_turn_exists() {
        let temp_dir = TempDir::new().expect("tempdir");
        let runtime = runtime(&temp_dir);
        runtime.register_session("thread-1".to_string(), None, temp_dir.path().to_path_buf());

        let response = runtime
            .handle_request_for_test(Request::ApplySteer(ApplySteerRequest {
                expected_turn_id: "turn-1".to_string(),
                text: "hello".to_string(),
                idempotency_key: "abc".to_string(),
                metadata: None,
            }))
            .await;
        let Response::ApplySteer { payload } = response else {
            panic!("expected apply-steer response");
        };
        assert_eq!(payload.outcome, ApplySteerResult::RejectedNoActiveTurn);
    }

    #[tokio::test]
    async fn apply_steer_rejects_stale_turns() {
        let temp_dir = TempDir::new().expect("tempdir");
        let runtime = runtime(&temp_dir);
        runtime.register_session("thread-1".to_string(), None, temp_dir.path().to_path_buf());
        runtime.note_turn_started("turn-2");

        let response = runtime
            .handle_request_for_test(Request::ApplySteer(ApplySteerRequest {
                expected_turn_id: "turn-1".to_string(),
                text: "hello".to_string(),
                idempotency_key: "abc".to_string(),
                metadata: None,
            }))
            .await;
        let Response::ApplySteer { payload } = response else {
            panic!("expected apply-steer response");
        };
        assert_eq!(payload.outcome, ApplySteerResult::RejectedStaleTurn);
        assert_eq!(payload.actual_turn_id.as_deref(), Some("turn-2"));
    }

    #[tokio::test]
    async fn apply_steer_rejects_duplicate_idempotency_keys() {
        let temp_dir = TempDir::new().expect("tempdir");
        let runtime = runtime(&temp_dir);
        runtime.register_session("thread-1".to_string(), None, temp_dir.path().to_path_buf());
        runtime.note_turn_started("turn-1");

        let first = runtime
            .handle_request_for_test(Request::ApplySteer(ApplySteerRequest {
                expected_turn_id: "turn-1".to_string(),
                text: "hello".to_string(),
                idempotency_key: "abc".to_string(),
                metadata: None,
            }))
            .await;
        let Response::ApplySteer { payload } = first else {
            panic!("expected apply-steer response");
        };
        assert_eq!(payload.outcome, ApplySteerResult::Applied);

        let second = runtime
            .handle_request_for_test(Request::ApplySteer(ApplySteerRequest {
                expected_turn_id: "turn-1".to_string(),
                text: "hello".to_string(),
                idempotency_key: "abc".to_string(),
                metadata: None,
            }))
            .await;
        let Response::ApplySteer { payload } = second else {
            panic!("expected apply-steer response");
        };
        assert_eq!(payload.outcome, ApplySteerResult::Duplicate);
        assert_eq!(payload.turn_id.as_deref(), Some("turn-1"));
    }

    #[tokio::test]
    async fn apply_steer_rejects_empty_text() {
        let temp_dir = TempDir::new().expect("tempdir");
        let runtime = runtime(&temp_dir);
        runtime.register_session("thread-1".to_string(), None, temp_dir.path().to_path_buf());
        runtime.note_turn_started("turn-1");

        let response = runtime
            .handle_request_for_test(Request::ApplySteer(ApplySteerRequest {
                expected_turn_id: "turn-1".to_string(),
                text: String::new(),
                idempotency_key: "abc".to_string(),
                metadata: None,
            }))
            .await;
        let Response::ApplySteer { payload } = response else {
            panic!("expected apply-steer response");
        };
        assert_eq!(payload.outcome, ApplySteerResult::Error);
        assert_eq!(payload.message.as_deref(), Some("text must not be empty"));
    }

    #[tokio::test]
    async fn apply_steer_surfaces_delegate_turn_mismatch() {
        let temp_dir = TempDir::new().expect("tempdir");
        let delegate = StubDelegate {
            result: Mutex::new(Err(SteerError::ExpectedTurnMismatch {
                actual_turn_id: "turn-2".to_string(),
            })),
        };
        let runtime =
            LocalControlPlane::start(config(&temp_dir), Arc::new(delegate)).expect("runtime");
        runtime.register_session("thread-1".to_string(), None, temp_dir.path().to_path_buf());
        runtime.note_turn_started("turn-1");

        let response = runtime
            .handle_request_for_test(Request::ApplySteer(ApplySteerRequest {
                expected_turn_id: "turn-1".to_string(),
                text: "hello".to_string(),
                idempotency_key: "abc".to_string(),
                metadata: None,
            }))
            .await;
        let Response::ApplySteer { payload } = response else {
            panic!("expected apply-steer response");
        };
        assert_eq!(payload.outcome, ApplySteerResult::RejectedStaleTurn);
        assert_eq!(payload.actual_turn_id.as_deref(), Some("turn-2"));
    }

    #[tokio::test]
    async fn socket_path_generation_uses_instance_id() {
        let temp_dir = TempDir::new().expect("tempdir");
        let runtime = runtime(&temp_dir);

        let socket_path = runtime.socket_path();
        assert_eq!(
            socket_path.parent(),
            Some(temp_dir.path().join("instances").as_path())
        );
        assert_eq!(
            socket_path.extension().and_then(|ext| ext.to_str()),
            Some("sock")
        );
        assert!(fs::exists(socket_path).expect("socket path should exist"));
    }
}
