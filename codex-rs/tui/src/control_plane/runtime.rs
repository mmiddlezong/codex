use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use codex_control_plane::ApplySteerRequest;
use codex_control_plane::LocalControlPlane;
use futures::SinkExt;
use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use url::Url;

use crate::app_event::ControlPlaneCreateChannelResult;

#[derive(Clone)]
pub(crate) struct ChannelServerClient {
    base_url: String,
    token: String,
    http_client: Client,
}

impl ChannelServerClient {
    pub(crate) fn new(base_url: String, token: String) -> Self {
        Self {
            base_url,
            token,
            http_client: Client::new(),
        }
    }

    pub(crate) async fn list_channels(&self) -> Result<Vec<String>, String> {
        let response = self
            .http_client
            .get(self.endpoint("/v1/channels")?)
            .header("authorization", format!("Bearer {}", self.token))
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
        let response = self
            .http_client
            .post(self.endpoint("/v1/channels")?)
            .header("authorization", format!("Bearer {}", self.token))
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
}

impl RemoteChannelWrapper {
    pub(crate) fn start(config: RemoteChannelWrapperConfig) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        tokio::spawn(run_wrapper(config, command_rx));
        Self { command_tx }
    }

    pub(crate) fn update_channels(&self, channels: Vec<String>) {
        let _ = self
            .command_tx
            .send(WrapperCommand::UpdateChannels(channels));
    }
}

async fn run_wrapper(
    config: RemoteChannelWrapperConfig,
    mut command_rx: mpsc::UnboundedReceiver<WrapperCommand>,
) {
    let mut channels = config.channels;
    let mut backoff_ms = 250u64;
    let mut warning_sent = false;

    loop {
        let websocket_url = match config.client.websocket_url() {
            Ok(websocket_url) => websocket_url,
            Err(err) => {
                send_wrapper_warning(&config.app_event_tx, err);
                return;
            }
        };
        let mut request = match websocket_url.as_str().into_client_request() {
            Ok(request) => request,
            Err(err) => {
                send_wrapper_warning(
                    &config.app_event_tx,
                    format!("failed to build wrapper websocket request: {err}"),
                );
                return;
            }
        };
        let authorization = match HeaderValue::from_str(&format!("Bearer {}", config.client.token))
        {
            Ok(authorization) => authorization,
            Err(err) => {
                send_wrapper_warning(
                    &config.app_event_tx,
                    format!("failed to encode wrapper authorization header: {err}"),
                );
                return;
            }
        };
        request.headers_mut().insert(AUTHORIZATION, authorization);

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
            text: text.to_string(),
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

fn send_wrapper_warning(app_event_tx: &AppEventSender, message: String) {
    app_event_tx.send(AppEvent::ControlPlaneWrapperWarning { message });
}
