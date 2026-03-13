mod channel_picker;
mod runtime;

use crate::app_event::AppEvent;
use crate::app_event::ControlPlaneFlowOrigin;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::custom_prompt_view::CustomPromptView;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::resume_picker::SessionSelection;
use async_trait::async_trait;
use codex_control_plane::ControlPlaneConfig;
use codex_control_plane::LaunchKind;
use codex_control_plane::LocalControlPlane;
use codex_control_plane::SteerDelegate;
use codex_control_plane::SteerError;
use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_core::config::edit::ConfigEditsBuilder;
use codex_core::config::types::ControlPlaneConsent;
use codex_protocol::ThreadId;
use codex_protocol::user_input::UserInput;
use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::sync::Arc;

pub(crate) use channel_picker::ChannelPickerView;
pub(crate) use runtime::ChannelServerClient;
pub(crate) use runtime::RemoteChannelWrapper;
pub(crate) use runtime::RemoteChannelWrapperConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChannelSelectionState {
    pub(crate) origin: ControlPlaneFlowOrigin,
    pub(crate) available_channels: Vec<String>,
    pub(crate) selected_channels: Vec<String>,
}

pub(crate) fn merge_available_channels(
    available_channels: &[String],
    selected_channels: &[String],
) -> Vec<String> {
    let mut merged = BTreeSet::new();
    for channel in available_channels {
        merged.insert(channel.clone());
    }
    for channel in selected_channels {
        merged.insert(channel.clone());
    }
    merged.into_iter().collect()
}

pub(crate) fn normalize_channel_name(input: &str) -> Result<String, String> {
    let normalized = input.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("Channel name cannot be empty.".to_string());
    }
    if normalized.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '.' | '_' | '-')
    }) {
        Ok(normalized)
    } else {
        Err(
            "Channel names may only contain lowercase letters, numbers, '.', '_' and '-'."
                .to_string(),
        )
    }
}

pub(crate) fn control_plane_consent_prompt() -> SelectionViewParams {
    let accept_action = Box::new(|tx: &AppEventSender| {
        tx.send(AppEvent::ControlPlaneConsentSelected {
            consent: ControlPlaneConsent::Accepted,
        });
    });
    let decline_action = Box::new(|tx: &AppEventSender| {
        tx.send(AppEvent::ControlPlaneConsentSelected {
            consent: ControlPlaneConsent::Declined,
        });
    });

    SelectionViewParams {
        title: Some("Local control-plane access".to_string()),
        subtitle: Some(
            "Allow a local process on this machine to inspect this Codex session and steer only the currently active turn."
                .to_string(),
        ),
        footer_note: Some(
            "This creates a per-instance local socket only after you opt in. Declining keeps the feature dormant and Codex will ask again on the next launch."
                .into(),
        ),
        footer_hint: Some(standard_popup_hint_line()),
        items: vec![
            SelectionItem {
                name: "Allow local control plane".to_string(),
                description: Some("Start the local IPC socket for this session.".to_string()),
                actions: vec![accept_action],
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Decline for now".to_string(),
                description: Some("Do not start the socket for this launch.".to_string()),
                actions: vec![decline_action],
                dismiss_on_select: true,
                ..Default::default()
            },
        ],
        on_cancel: Some(Box::new(|tx| {
            tx.send(AppEvent::ControlPlaneConsentSelected {
                consent: ControlPlaneConsent::Declined,
            });
        })),
        ..Default::default()
    }
}

pub(crate) fn launch_kind_from_session_selection(
    session_selection: &SessionSelection,
) -> LaunchKind {
    match session_selection {
        SessionSelection::StartFresh | SessionSelection::Exit => LaunchKind::Fresh,
        SessionSelection::Resume(_) => LaunchKind::Resume,
        SessionSelection::Fork(_) => LaunchKind::Fork,
    }
}

pub(crate) async fn persist_control_plane_consent(
    codex_home: &Path,
    consent: ControlPlaneConsent,
) -> io::Result<()> {
    ConfigEditsBuilder::new(codex_home)
        .set_control_plane_consent(consent)
        .apply()
        .await
        .map_err(|err| io::Error::other(format!("failed to persist control-plane consent: {err}")))
}

pub(crate) fn should_prompt_for_control_plane_consent(config: &Config) -> bool {
    config.control_plane.enabled
        && !matches!(
            config.control_plane.consent,
            Some(ControlPlaneConsent::Accepted)
        )
}

pub(crate) fn should_prompt_for_control_plane_setup(config: &Config) -> bool {
    config.control_plane.enabled
        && matches!(
            config.control_plane.consent,
            Some(ControlPlaneConsent::Accepted)
        )
        && (config.control_plane.server_url.is_none()
            || config.control_plane.server_token.is_none())
}

pub(crate) fn control_plane_server_url_prompt(app_event_tx: AppEventSender) -> CustomPromptView {
    CustomPromptView::new(
        "Channel server URL".to_string(),
        "Type the server base URL and press Enter".to_string(),
        Some("Example: http://127.0.0.1:3000".to_string()),
        Box::new(move |url| {
            app_event_tx.send(AppEvent::ControlPlaneServerUrlSubmitted { url });
        }),
    )
}

pub(crate) fn control_plane_server_token_prompt(app_event_tx: AppEventSender) -> CustomPromptView {
    CustomPromptView::new(
        "Channel server token".to_string(),
        "Paste the bearer token and press Enter".to_string(),
        Some("This will be stored in [control_plane].server_token.".to_string()),
        Box::new(move |token| {
            app_event_tx.send(AppEvent::ControlPlaneServerTokenSubmitted { token });
        }),
    )
}

pub(crate) fn control_plane_create_channel_prompt(
    app_event_tx: AppEventSender,
) -> CustomPromptView {
    CustomPromptView::new(
        "Create channel".to_string(),
        "Type a new channel name and press Enter".to_string(),
        Some("Allowed: lowercase letters, numbers, '.', '_' and '-'.".to_string()),
        Box::new(move |name| {
            app_event_tx.send(AppEvent::ControlPlaneCreateChannelSubmitted { name });
        }),
    )
}

pub(crate) fn control_plane_save_channels_prompt(channels: &[String]) -> SelectionViewParams {
    let save_channels = channels.to_vec();
    let keep_channels = channels.to_vec();
    SelectionViewParams {
        title: Some("Save channel selection?".to_string()),
        subtitle: Some(format!(
            "Use {} for this run only, or also save them for future runs.",
            if channels.is_empty() {
                "no channels".to_string()
            } else {
                channels.join(", ")
            }
        )),
        footer_hint: Some(standard_popup_hint_line()),
        items: vec![
            SelectionItem {
                name: "Yes, save".to_string(),
                description: Some("Persist this channel selection to config.toml.".to_string()),
                actions: vec![Box::new(move |tx| {
                    let _ = &save_channels;
                    tx.send(AppEvent::ControlPlaneSaveChannelSelection { save: true });
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "No, this run only".to_string(),
                description: Some(
                    "Use these channels now without changing config.toml.".to_string(),
                ),
                actions: vec![Box::new(move |tx| {
                    let _ = &keep_channels;
                    tx.send(AppEvent::ControlPlaneSaveChannelSelection { save: false });
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
        ],
        on_cancel: Some(Box::new(|tx| {
            tx.send(AppEvent::ControlPlaneSaveChannelSelection { save: false });
        })),
        ..Default::default()
    }
}

pub(crate) fn start_control_plane(
    config: &Config,
    thread_manager: Arc<ThreadManager>,
    launch_kind: LaunchKind,
) -> io::Result<LocalControlPlane> {
    LocalControlPlane::start(
        ControlPlaneConfig {
            feature_enabled: config.control_plane.enabled,
            consent_accepted: matches!(
                config.control_plane.consent,
                Some(ControlPlaneConsent::Accepted)
            ),
            steering_enabled: config.control_plane.steering_enabled,
            ipc_dir: config.control_plane.ipc_dir.clone(),
            launch_kind,
        },
        Arc::new(TuiSteerDelegate { thread_manager }),
    )
}

struct TuiSteerDelegate {
    thread_manager: Arc<ThreadManager>,
}

#[async_trait]
impl SteerDelegate for TuiSteerDelegate {
    async fn apply_steer(
        &self,
        thread_id: &str,
        expected_turn_id: &str,
        text: String,
    ) -> Result<String, SteerError> {
        let thread_id_string = thread_id.to_string();
        let thread_id = ThreadId::from_string(thread_id)
            .map_err(|err| SteerError::Error(format!("invalid thread id `{thread_id}`: {err}")))?;
        let thread = self
            .thread_manager
            .get_thread(thread_id)
            .await
            .map_err(|err| {
                SteerError::Error(format!("failed to load thread `{thread_id_string}`: {err}"))
            })?;

        thread
            .steer_input(
                vec![UserInput::Text {
                    text,
                    text_elements: Vec::new(),
                }],
                Some(expected_turn_id),
            )
            .await
            .map_err(|err| match err {
                codex_core::SteerInputError::NoActiveTurn(_) => SteerError::NoActiveTurn,
                codex_core::SteerInputError::ExpectedTurnMismatch { actual, .. } => {
                    SteerError::ExpectedTurnMismatch {
                        actual_turn_id: actual,
                    }
                }
                codex_core::SteerInputError::EmptyInput => {
                    SteerError::Error("text must not be empty".to_string())
                }
            })
    }
}
