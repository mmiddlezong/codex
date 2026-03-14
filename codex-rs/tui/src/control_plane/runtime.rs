use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::control_plane::server_headers::apply_channel_server_http_headers;
use crate::control_plane::server_headers::apply_channel_server_websocket_headers;
use crate::control_plane::server_headers::build_channel_server_http_headers;
use codex_control_plane::ApplySteerRequest;
use codex_control_plane::LocalControlPlane;
use futures::SinkExt;
use futures::StreamExt;
use reqwest::Client;
use reqwest::header::AUTHORIZATION;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;

use crate::app_event::ControlPlaneCreateChannelResult;

const CHANNEL_PLACEHOLDER: &str = "{channel}";
const CONTENTS_PLACEHOLDER: &str = "{contents}";
pub(crate) const DEFAULT_CHANNEL_STEER_MESSAGE_TEMPLATE: &str = "You have received a message from another Codex instance in the channel #{channel}. Please continue working after reading this message. You do not need to stop.\n\nContents of the message:\n{contents}";

#[derive(Clone)]
pub(crate) struct ChannelServerClient {
    base_url: String,
    token: String,
    http_client: Client,
    http_headers: HeaderMap,
}

impl ChannelServerClient {
    pub(crate) fn new(
        base_url: String,
        token: String,
        http_headers: Option<HashMap<String, String>>,
    ) -> Result<Self, String> {
        let http_headers = build_channel_server_http_headers(http_headers)?;
        Ok(Self {
            base_url,
            token,
            http_client: Client::new(),
            http_headers,
        })
    }

    pub(crate) async fn list_channels(&self) -> Result<Vec<String>, String> {
        let response = apply_channel_server_http_headers(
            self.http_client.get(self.endpoint("/v1/channels")?),
            &self.http_headers,
        )
        .header(AUTHORIZATION, format!("Bearer {}", self.token))
        .send()
        .await
        .map_err(|err| format!("failed to fetch channels: {err}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "channel list request failed: HTTP {}",
                response.status()
            ));
        }

        let payload: ListChannelsResponse = response
            .json()
            .await
            .map_err(|err| format!("failed to decode channel list: {err}"))?;
        Ok(payload.channels)
    }

    pub(crate) async fn create_channel(
        &self,
        channel: &str,
    ) -> Result<ControlPlaneCreateChannelResult, String> {
        let response = apply_channel_server_http_headers(
            self.http_client.post(self.endpoint("/v1/channels")?),
            &self.http_headers,
        )
        .header(AUTHORIZATION, format!("Bearer {}", self.token))
        .json(&CreateChannelRequest {
            channel: channel.to_string(),
        })
        .send()
        .await
        .map_err(|err| format!("failed to create channel `{channel}`: {err}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "channel create request failed for `{channel}`: HTTP {}",
                response.status()
            ));
        }

        let payload: CreateChannelResponse = response
            .json()
            .await
            .map_err(|err| format!("failed to decode channel create response: {err}"))?;
        Ok(ControlPlaneCreateChannelResult {
            channel: payload.channel,
            created: payload.created,
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url, String> {
        Url::parse(&self.base_url)
            .map_err(|err| format!("invalid server URL `{}`: {err}", self.base_url))
            .and_then(|base_url| {
                base_url
                    .join(path)
                    .map_err(|err| format!("invalid endpoint path `{path}`: {err}"))
            })
    }

    fn websocket_url(&self) -> Result<Url, String> {
        let mut url = self.endpoint("/v1/subscribe")?;
        match url.scheme() {
            "http" => {
                let _ = url.set_scheme("ws");
            }
            "https" => {
                let _ = url.set_scheme("wss");
            }
            "ws" | "wss" => {}
            _ => return Err(format!("unsupported server URL scheme `{}`", url.scheme())),
        }
        Ok(url)
    }

    fn websocket_request(
        &self,
    ) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
        let websocket_url = self.websocket_url()?;
        let mut request = websocket_url
            .as_str()
            .into_client_request()
            .map_err(|err| format!("failed to build wrapper websocket request: {err}"))?;
        apply_channel_server_websocket_headers(&mut request, &self.http_headers);

        let authorization = HeaderValue::from_str(&format!("Bearer {}", self.token))
            .map_err(|err| format!("failed to encode wrapper authorization header: {err}"))?;
        request.headers_mut().insert(AUTHORIZATION, authorization);
        Ok(request)
    }
}

#[derive(Serialize)]
struct HelloMessage {
    r#type: &'static str,
    #[serde(rename = "wrapperId")]
    wrapper_id: String,
    channels: Vec<String>,
    #[serde(rename = "instanceId")]
    instance_id: String,
    hostname: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMessage {
    Subscribed {},
    ChannelMessage {
        #[serde(rename = "messageId")]
        message_id: String,
        channel: String,
        text: String,
        metadata: Option<serde_json::Value>,
        #[serde(rename = "createdAt")]
        created_at: String,
    },
}

#[derive(Serialize)]
struct MessageResult {
    r#type: &'static str,
    #[serde(rename = "messageId")]
    message_id: String,
    outcome: &'static str,
    #[serde(rename = "turnId", skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Deserialize)]
struct ListChannelsResponse {
    channels: Vec<String>,
}

#[derive(Serialize)]
struct CreateChannelRequest {
    channel: String,
}

#[derive(Deserialize)]
struct CreateChannelResponse {
    channel: String,
    created: bool,
}

enum WrapperCommand {
    UpdateChannels(Vec<String>),
}

pub(crate) struct RemoteChannelWrapper {
    command_tx: mpsc::UnboundedSender<WrapperCommand>,
}

pub(crate) struct RemoteChannelWrapperConfig {
    pub(crate) client: ChannelServerClient,
    pub(crate) local_control_plane: Arc<LocalControlPlane>,
    pub(crate) app_event_tx: AppEventSender,
    pub(crate) channels: Vec<String>,
    pub(crate) wrapper_id: String,
    pub(crate) instance_id: String,
    pub(crate) hostname: String,
    pub(crate) label: Option<String>,
    pub(crate) steer_message_template: Option<String>,
}

impl RemoteChannelWrapper {
    pub(crate) fn start(config: RemoteChannelWrapperConfig) -> Option<Self> {
        let steer_message_template = match resolve_channel_steer_message_template(
            config.steer_message_template.as_deref(),
        ) {
            Ok(template) => template,
            Err(err) => {
                send_wrapper_warning(
                    &config.app_event_tx,
                    format!(
                        "channel wrapper disabled because control_plane.steer_message_template is invalid: {err}"
                    ),
                );
                return None;
            }
        };
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        tokio::spawn(run_wrapper(config, steer_message_template, command_rx));
        Some(Self { command_tx })
    }

    pub(crate) fn update_channels(&self, channels: Vec<String>) {
        let _ = self
            .command_tx
            .send(WrapperCommand::UpdateChannels(channels));
    }
}

async fn run_wrapper(
    config: RemoteChannelWrapperConfig,
    steer_message_template: String,
    mut command_rx: mpsc::UnboundedReceiver<WrapperCommand>,
) {
    let mut channels = config.channels;
    let mut backoff_ms = 250u64;
    let mut warning_sent = false;

    loop {
        let request = match config.client.websocket_request() {
            Ok(request) => request,
            Err(err) => {
                send_wrapper_warning(&config.app_event_tx, err);
                return;
            }
        };

        match connect_async(request).await {
            Ok((mut socket, _response)) => {
                warning_sent = false;
                backoff_ms = 250;

                let hello = HelloMessage {
                    r#type: "hello",
                    wrapper_id: config.wrapper_id.clone(),
                    channels: channels.clone(),
                    instance_id: config.instance_id.clone(),
                    hostname: config.hostname.clone(),
                    label: config.label.clone(),
                };
                if let Err(err) = socket
                    .send(Message::Text(
                        serde_json::to_string(&hello).unwrap_or_default().into(),
                    ))
                    .await
                {
                    send_wrapper_warning(
                        &config.app_event_tx,
                        format!("failed to send wrapper hello: {err}"),
                    );
                    sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(5_000);
                    continue;
                }

                let mut subscribed = false;

                loop {
                    tokio::select! {
                        maybe_command = command_rx.recv() => {
                            match maybe_command {
                                Some(WrapperCommand::UpdateChannels(next_channels)) => {
                                    channels = next_channels;
                                    let _ = socket.close(None).await;
                                    break;
                                }
                                None => {
                                    let _ = socket.close(None).await;
                                    return;
                                }
                            }
                        }
                        maybe_message = socket.next() => {
                            match maybe_message {
                                Some(Ok(Message::Text(text))) => {
                                    let parsed = serde_json::from_str::<ServerMessage>(&text);
                                    match parsed {
                                        Ok(ServerMessage::Subscribed { .. }) => {
                                            subscribed = true;
                                        }
                                        Ok(ServerMessage::ChannelMessage { message_id, channel, text, metadata, created_at: _created_at }) => {
                                            let receipt = build_receipt(
                                                &config.local_control_plane,
                                                &steer_message_template,
                                                &message_id,
                                                &channel,
                                                text.as_str(),
                                                metadata,
                                            )
                                            .await;
                                            let serialized = serde_json::to_string(&receipt).unwrap_or_default();
                                            if let Err(err) = socket.send(Message::Text(serialized.into())).await {
                                                send_wrapper_warning(
                                                    &config.app_event_tx,
                                                    format!("failed to send wrapper receipt: {err}"),
                                                );
                                                break;
                                            }
                                        }
                                        Err(err) => {
                                            send_wrapper_warning(
                                                &config.app_event_tx,
                                                format!("failed to decode wrapper message: {err}"),
                                            );
                                        }
                                    }
                                }
                                Some(Ok(Message::Ping(payload))) => {
                                    let _ = socket.send(Message::Pong(payload)).await;
                                }
                                Some(Ok(Message::Close(_))) | None => {
                                    if subscribed && !warning_sent {
                                        send_wrapper_warning(
                                            &config.app_event_tx,
                                            "channel wrapper disconnected; reconnecting in background".to_string(),
                                        );
                                        warning_sent = true;
                                    }
                                    break;
                                }
                                Some(Ok(_)) => {}
                                Some(Err(err)) => {
                                    if !warning_sent {
                                        send_wrapper_warning(
                                            &config.app_event_tx,
                                            format!("channel wrapper websocket error: {err}"),
                                        );
                                        warning_sent = true;
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Err(err) => {
                if !warning_sent {
                    send_wrapper_warning(
                        &config.app_event_tx,
                        format!("failed to connect channel wrapper: {err}"),
                    );
                    warning_sent = true;
                }
            }
        }

        tokio::select! {
            maybe_command = command_rx.recv() => {
                match maybe_command {
                    Some(WrapperCommand::UpdateChannels(next_channels)) => {
                        channels = next_channels;
                    }
                    None => {
                        return;
                    }
                }
            }
            _ = sleep(Duration::from_millis(backoff_ms)) => {}
        }
        backoff_ms = (backoff_ms * 2).min(5_000);
    }
}

async fn build_receipt(
    local_control_plane: &Arc<LocalControlPlane>,
    steer_message_template: &str,
    message_id: &str,
    channel: &str,
    text: &str,
    metadata: Option<serde_json::Value>,
) -> MessageResult {
    let Some(current_turn) = local_control_plane.current_turn() else {
        return MessageResult {
            r#type: "message_result",
            message_id: message_id.to_string(),
            outcome: "skipped_no_active_turn",
            turn_id: None,
            detail: None,
        };
    };

    let steer_result = local_control_plane
        .apply_steer(ApplySteerRequest {
            expected_turn_id: current_turn.turn_id.clone(),
            text: format_channel_steer_message(steer_message_template, channel, text),
            idempotency_key: message_id.to_string(),
            metadata: Some(json!({
                "channel": channel,
                "messageId": message_id,
                "serverMetadata": metadata,
            })),
        })
        .await;

    match steer_result.outcome {
        codex_control_plane::ApplySteerResult::Applied
        | codex_control_plane::ApplySteerResult::Duplicate => MessageResult {
            r#type: "message_result",
            message_id: message_id.to_string(),
            outcome: "steered",
            turn_id: steer_result.turn_id.or(Some(current_turn.turn_id)),
            detail: None,
        },
        codex_control_plane::ApplySteerResult::RejectedNoActiveTurn => MessageResult {
            r#type: "message_result",
            message_id: message_id.to_string(),
            outcome: "skipped_no_active_turn",
            turn_id: None,
            detail: None,
        },
        codex_control_plane::ApplySteerResult::RejectedStaleTurn => MessageResult {
            r#type: "message_result",
            message_id: message_id.to_string(),
            outcome: "skipped_stale_turn",
            turn_id: steer_result.actual_turn_id,
            detail: steer_result.message,
        },
        _ => MessageResult {
            r#type: "message_result",
            message_id: message_id.to_string(),
            outcome: "error",
            turn_id: None,
            detail: steer_result.message,
        },
    }
}

pub(crate) fn resolve_channel_steer_message_template(
    template: Option<&str>,
) -> Result<String, String> {
    let template = template.unwrap_or(DEFAULT_CHANNEL_STEER_MESSAGE_TEMPLATE);
    validate_channel_steer_message_template(template)?;
    Ok(template.to_string())
}

fn validate_channel_steer_message_template(template: &str) -> Result<(), String> {
    if !template.contains(CHANNEL_PLACEHOLDER) {
        return Err(format!(
            "missing required placeholder `{CHANNEL_PLACEHOLDER}`"
        ));
    }
    if !template.contains(CONTENTS_PLACEHOLDER) {
        return Err(format!(
            "missing required placeholder `{CONTENTS_PLACEHOLDER}`"
        ));
    }

    let bytes = template.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => {
                let Some(end_rel) = template[index + 1..].find('}') else {
                    return Err("unterminated placeholder in steer_message_template".to_string());
                };
                let end = index + 1 + end_rel;
                let placeholder = &template[index..=end];
                if placeholder != CHANNEL_PLACEHOLDER && placeholder != CONTENTS_PLACEHOLDER {
                    return Err(format!(
                        "unsupported placeholder `{placeholder}` in steer_message_template"
                    ));
                }
                index = end + 1;
            }
            b'}' => {
                return Err("unmatched `}` in steer_message_template".to_string());
            }
            _ => {
                index += 1;
            }
        }
    }

    Ok(())
}

fn format_channel_steer_message(template: &str, channel: &str, text: &str) -> String {
    let quoted_message = text
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    template
        .replace(CHANNEL_PLACEHOLDER, channel)
        .replace(CONTENTS_PLACEHOLDER, &quoted_message)
}

fn send_wrapper_warning(app_event_tx: &AppEventSender, message: String) {
    app_event_tx.send(AppEvent::ControlPlaneWrapperWarning { message });
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use codex_control_plane::ControlPlaneConfig;
    use codex_control_plane::LaunchKind;
    use codex_control_plane::SteerDelegate;
    use codex_control_plane::SteerError;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tempfile::tempdir;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn format_channel_steer_message_single_line() {
        assert_eq!(
            format_channel_steer_message(
                DEFAULT_CHANNEL_STEER_MESSAGE_TEMPLATE,
                "ops",
                "hello world"
            ),
            "You have received a message from another Codex instance in the channel #ops. Please continue working after reading this message. You do not need to stop.\n\nContents of the message:\n> hello world"
        );
    }

    #[test]
    fn format_channel_steer_message_multiline() {
        assert_eq!(
            format_channel_steer_message(
                DEFAULT_CHANNEL_STEER_MESSAGE_TEMPLATE,
                "ops",
                "line one\nline two"
            ),
            "You have received a message from another Codex instance in the channel #ops. Please continue working after reading this message. You do not need to stop.\n\nContents of the message:\n> line one\n> line two"
        );
    }

    #[test]
    fn format_channel_steer_message_preserves_blank_lines() {
        assert_eq!(
            format_channel_steer_message(
                DEFAULT_CHANNEL_STEER_MESSAGE_TEMPLATE,
                "ops",
                "line one\n\nline three"
            ),
            "You have received a message from another Codex instance in the channel #ops. Please continue working after reading this message. You do not need to stop.\n\nContents of the message:\n> line one\n>\n> line three"
        );
    }

    #[test]
    fn format_channel_steer_message_preserves_whitespace() {
        assert_eq!(
            format_channel_steer_message(
                DEFAULT_CHANNEL_STEER_MESSAGE_TEMPLATE,
                "ops",
                "  padded  \ntrailing  "
            ),
            "You have received a message from another Codex instance in the channel #ops. Please continue working after reading this message. You do not need to stop.\n\nContents of the message:\n>   padded  \n> trailing  "
        );
    }

    #[test]
    fn resolve_channel_steer_message_template_accepts_default_template() {
        assert_eq!(
            resolve_channel_steer_message_template(None).as_deref(),
            Ok(DEFAULT_CHANNEL_STEER_MESSAGE_TEMPLATE)
        );
    }

    #[test]
    fn resolve_channel_steer_message_template_rejects_missing_channel_placeholder() {
        let expected = "missing required placeholder `{channel}`".to_string();
        assert_eq!(
            resolve_channel_steer_message_template(Some("Hello\n{contents}")).as_deref(),
            Err(&expected)
        );
    }

    #[test]
    fn resolve_channel_steer_message_template_rejects_missing_contents_placeholder() {
        let expected = "missing required placeholder `{contents}`".to_string();
        assert_eq!(
            resolve_channel_steer_message_template(Some("Hello #{channel}")).as_deref(),
            Err(&expected)
        );
    }

    #[test]
    fn resolve_channel_steer_message_template_rejects_unknown_placeholder() {
        let expected = "unsupported placeholder `{oops}` in steer_message_template".to_string();
        assert_eq!(
            resolve_channel_steer_message_template(Some("{channel}\n{contents}\n{oops}"))
                .as_deref(),
            Err(&expected)
        );
    }

    #[test]
    fn resolve_channel_steer_message_template_allows_repeated_placeholders() {
        assert_eq!(
            resolve_channel_steer_message_template(Some("{channel}\n{contents}\n{channel}"))
                .as_deref(),
            Ok("{channel}\n{contents}\n{channel}")
        );
    }

    #[derive(Default)]
    struct RecordingDelegate {
        requests: Mutex<Vec<(String, String, String)>>,
    }

    #[async_trait]
    impl SteerDelegate for RecordingDelegate {
        async fn apply_steer(
            &self,
            thread_id: &str,
            expected_turn_id: &str,
            text: String,
        ) -> Result<String, SteerError> {
            self.requests.lock().expect("lock").push((
                thread_id.to_string(),
                expected_turn_id.to_string(),
                text,
            ));
            Ok("turn-1".to_string())
        }
    }

    fn test_control_plane(delegate: Arc<dyn SteerDelegate>) -> Arc<LocalControlPlane> {
        let temp_dir = tempdir().expect("tempdir");
        let control_plane = LocalControlPlane::start(
            ControlPlaneConfig {
                feature_enabled: true,
                consent_accepted: true,
                steering_enabled: true,
                ipc_dir: temp_dir.path().join("instances"),
                launch_kind: LaunchKind::Fresh,
            },
            delegate,
        )
        .expect("control plane");
        let control_plane = Arc::new(control_plane);
        control_plane.register_session(
            "thread-1".to_string(),
            Some("demo".to_string()),
            PathBuf::from("/tmp/project"),
        );
        control_plane.note_turn_started("turn-1");
        control_plane
    }

    #[tokio::test]
    async fn build_receipt_uses_templated_steer_text() {
        let delegate = Arc::new(RecordingDelegate::default());
        let control_plane = test_control_plane(delegate.clone());

        let receipt = build_receipt(
            &control_plane,
            DEFAULT_CHANNEL_STEER_MESSAGE_TEMPLATE,
            "msg-1",
            "ops",
            "hello\n\nworld",
            Some(json!({"source": "test"})),
        )
        .await;

        assert_eq!(receipt.r#type, "message_result");
        assert_eq!(receipt.message_id, "msg-1");
        assert_eq!(receipt.outcome, "steered");
        assert_eq!(receipt.turn_id, Some("turn-1".to_string()));
        assert_eq!(receipt.detail, None);

        let requests = delegate.requests.lock().expect("lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "thread-1");
        assert_eq!(requests[0].1, "turn-1");
        assert_eq!(
            requests[0].2,
            "You have received a message from another Codex instance in the channel #ops. Please continue working after reading this message. You do not need to stop.\n\nContents of the message:\n> hello\n>\n> world"
        );
        std::mem::forget(control_plane);
    }

    #[tokio::test]
    async fn invalid_template_prevents_wrapper_start_and_emits_warning() {
        let delegate = Arc::new(RecordingDelegate::default());
        let control_plane = test_control_plane(delegate);
        let (tx_raw, mut rx) = unbounded_channel();
        let app_event_tx = AppEventSender::new(tx_raw);

        let wrapper = RemoteChannelWrapper::start(RemoteChannelWrapperConfig {
            client: ChannelServerClient::new(
                "http://127.0.0.1:3000".to_string(),
                "token".to_string(),
                None,
            )
            .expect("client"),
            local_control_plane: control_plane.clone(),
            app_event_tx,
            channels: vec!["ops".to_string()],
            wrapper_id: "wrapper-1".to_string(),
            instance_id: "instance-1".to_string(),
            hostname: "host".to_string(),
            label: None,
            steer_message_template: Some("Hello {channel}".to_string()),
        });

        assert_eq!(wrapper.is_none(), true);
        let event = rx.recv().await.expect("wrapper warning event");
        match event {
            AppEvent::ControlPlaneWrapperWarning { message } => {
                assert_eq!(
                    message,
                    "channel wrapper disabled because control_plane.steer_message_template is invalid: missing required placeholder `{contents}`"
                );
            }
            other => panic!("expected wrapper warning event, got {other:?}"),
        }
        std::mem::forget(control_plane);
    }
}
