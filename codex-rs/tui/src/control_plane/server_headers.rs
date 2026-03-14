use reqwest::RequestBuilder;
use reqwest::header::AUTHORIZATION;
use reqwest::header::CONTENT_TYPE;
use reqwest::header::HOST;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use std::collections::HashMap;
use tokio_tungstenite::tungstenite::http::Request;

pub(crate) fn build_channel_server_http_headers(
    http_headers: Option<HashMap<String, String>>,
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();

    if let Some(http_headers) = http_headers {
        for (name, value) in http_headers {
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|err| {
                format!("control_plane.http_headers contains invalid header name `{name}`: {err}")
            })?;
            if is_reserved_channel_server_header(&header_name) {
                return Err(format!(
                    "control_plane.http_headers cannot override reserved header `{name}`"
                ));
            }

            let header_value = HeaderValue::from_str(value.as_str()).map_err(|err| {
                format!(
                    "control_plane.http_headers contains invalid value for `{name}`: {err}"
                )
            })?;
            headers.insert(header_name, header_value);
        }
    }

    Ok(headers)
}

pub(crate) fn apply_channel_server_http_headers(
    request_builder: RequestBuilder,
    http_headers: &HeaderMap,
) -> RequestBuilder {
    http_headers
        .iter()
        .fold(request_builder, |builder, (name, value)| {
            builder.header(name, value)
        })
}

pub(crate) fn apply_channel_server_websocket_headers<T>(
    request: &mut Request<T>,
    http_headers: &HeaderMap,
) {
    for (name, value) in http_headers {
        request.headers_mut().insert(name.clone(), value.clone());
    }
}

fn is_reserved_channel_server_header(header_name: &HeaderName) -> bool {
    header_name == AUTHORIZATION || header_name == CONTENT_TYPE || header_name == HOST
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    #[test]
    fn build_channel_server_http_headers_accepts_custom_headers() {
        let headers = build_channel_server_http_headers(Some(HashMap::from([
            (
                "CF-Access-Client-Id".to_string(),
                "client-id".to_string(),
            ),
            (
                "CF-Access-Client-Secret".to_string(),
                "client-secret".to_string(),
            ),
        ])))
        .expect("headers");

        let client_id = HeaderValue::from_static("client-id");
        let client_secret = HeaderValue::from_static("client-secret");
        assert_eq!(headers.get("cf-access-client-id"), Some(&client_id));
        assert_eq!(headers.get("cf-access-client-secret"), Some(&client_secret));
    }

    #[test]
    fn build_channel_server_http_headers_rejects_reserved_headers() {
        for header_name in ["authorization", "content-type", "host"] {
            let err = build_channel_server_http_headers(Some(HashMap::from([(
                header_name.to_string(),
                "reserved".to_string(),
            )])))
            .expect_err("reserved headers should be rejected");

            assert_eq!(
                err,
                format!(
                    "control_plane.http_headers cannot override reserved header `{header_name}`"
                )
            );
        }
    }

    #[test]
    fn build_channel_server_http_headers_rejects_invalid_header_name() {
        let err = build_channel_server_http_headers(Some(HashMap::from([(
            "bad header".to_string(),
            "value".to_string(),
        )])))
        .expect_err("invalid header name should be rejected");

        assert_eq!(
            err,
            "control_plane.http_headers contains invalid header name `bad header`: invalid HTTP header name"
        );
    }

    #[test]
    fn apply_channel_server_http_headers_adds_headers_to_http_requests() {
        let headers = build_channel_server_http_headers(Some(HashMap::from([(
            "CF-Access-Client-Id".to_string(),
            "client-id".to_string(),
        )])))
        .expect("headers");

        let request = apply_channel_server_http_headers(
            reqwest::Client::new().get("https://example.invalid/v1/channels"),
            &headers,
        )
        .build()
        .expect("request");

        let expected = HeaderValue::from_static("client-id");
        assert_eq!(request.headers().get("cf-access-client-id"), Some(&expected));
    }

    #[test]
    fn apply_channel_server_websocket_headers_adds_headers_to_websocket_requests() {
        let headers = build_channel_server_http_headers(Some(HashMap::from([(
            "CF-Access-Client-Id".to_string(),
            "client-id".to_string(),
        )])))
        .expect("headers");
        let mut request = "wss://example.invalid/v1/subscribe"
            .into_client_request()
            .expect("request");

        apply_channel_server_websocket_headers(&mut request, &headers);

        let expected = HeaderValue::from_static("client-id");
        assert_eq!(request.headers().get("cf-access-client-id"), Some(&expected));
    }
}
