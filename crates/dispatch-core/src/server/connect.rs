//! The Connect protocol on axum: unary calls in JSON or binary protobuf, and
//! server streams in enveloped JSON or protobuf, as connect-go serves them.

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::Stream;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

/// A Connect error: its code and a message for the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    pub code: &'static str,
    pub message: String,
}

impl RpcError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid_argument", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    pub fn internal(message: impl std::fmt::Display) -> Self {
        Self::new("internal", message.to_string())
    }

    pub fn unauthenticated(message: impl Into<String>) -> Self {
        Self::new("unauthenticated", message)
    }

    fn status(&self) -> StatusCode {
        let code = match self.code {
            "canceled" => 499,
            "invalid_argument" | "failed_precondition" | "out_of_range" => 400,
            "deadline_exceeded" => 504,
            "not_found" => 404,
            "already_exists" | "aborted" => 409,
            "permission_denied" => 403,
            "resource_exhausted" => 429,
            "unimplemented" => 501,
            "unavailable" => 503,
            "unauthenticated" => 401,
            _ => 500,
        };
        StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }

    fn wire(&self) -> serde_json::Value {
        if self.message.is_empty() {
            json!({"code": self.code})
        } else {
            json!({"code": self.code, "message": self.message})
        }
    }
}

impl IntoResponse for RpcError {
    fn into_response(self) -> Response {
        let body = serde_json::to_vec(&self.wire()).unwrap_or_default();
        (
            self.status(),
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            body,
        )
            .into_response()
    }
}

pub type RpcResult<T> = Result<T, RpcError>;

/// The message codec a call asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Json,
    Proto,
}

impl Codec {
    fn of(headers: &HeaderMap, streaming: bool) -> RpcResult<Self> {
        let ct = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let want = if streaming {
            ct.strip_prefix("application/connect+")
        } else {
            ct.strip_prefix("application/")
        };
        match want {
            Some("json") => Ok(Codec::Json),
            Some("proto") => Ok(Codec::Proto),
            _ => Err(RpcError::new(
                "unimplemented",
                format!("unsupported content type {ct:?}"),
            )),
        }
    }

    fn decode<T: prost::Message + DeserializeOwned + Default>(self, b: &[u8]) -> RpcResult<T> {
        match self {
            Codec::Json if b.is_empty() => Ok(T::default()),
            Codec::Json => serde_json::from_slice(b)
                .map_err(|e| RpcError::invalid(format!("unmarshal message: {e}"))),
            Codec::Proto => {
                T::decode(b).map_err(|e| RpcError::invalid(format!("unmarshal message: {e}")))
            }
        }
    }

    fn encode<T: prost::Message + Serialize>(self, m: &T) -> RpcResult<Vec<u8>> {
        match self {
            Codec::Json => serde_json::to_vec(m).map_err(RpcError::internal),
            Codec::Proto => Ok(m.encode_to_vec()),
        }
    }

    fn content_type(self, streaming: bool) -> &'static str {
        match (self, streaming) {
            (Codec::Json, false) => "application/json",
            (Codec::Proto, false) => "application/proto",
            (Codec::Json, true) => "application/connect+json",
            (Codec::Proto, true) => "application/connect+proto",
        }
    }
}

fn compressed(headers: &HeaderMap, name: &str) -> bool {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.is_empty() && v != "identity")
}

/// Decodes a unary request in the codec it names.
pub fn unary_request<T: prost::Message + DeserializeOwned + Default>(
    headers: &HeaderMap,
    body: &Bytes,
) -> RpcResult<(Codec, T)> {
    let codec = Codec::of(headers, false)?;
    if compressed(headers, "content-encoding") {
        return Err(RpcError::new(
            "unimplemented",
            "compressed requests are not supported",
        ));
    }
    Ok((codec, codec.decode(body)?))
}

/// Encodes a unary answer, or its error.
pub fn unary_response<T: prost::Message + Serialize>(codec: Codec, r: RpcResult<T>) -> Response {
    match r.and_then(|m| codec.encode(&m)) {
        Ok(body) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static(codec.content_type(false)),
            )],
            body,
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

fn envelope(flags: u8, payload: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(flags);
    out.extend_from_slice(
        &u32::try_from(payload.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    out.extend_from_slice(payload);
    Bytes::from(out)
}

/// Decodes the one enveloped message of a server-streaming request.
pub fn stream_request<T: prost::Message + DeserializeOwned + Default>(
    headers: &HeaderMap,
    body: &Bytes,
) -> RpcResult<(Codec, T)> {
    let codec = Codec::of(headers, true)?;
    if body.len() < 5 {
        return Err(RpcError::invalid("missing request envelope"));
    }
    if body[0] & 0x01 != 0 {
        return Err(RpcError::new(
            "unimplemented",
            "compressed messages are not supported",
        ));
    }
    let len = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize;
    let payload = body
        .get(5..5 + len)
        .ok_or_else(|| RpcError::invalid("truncated request envelope"))?;
    Ok((codec, codec.decode(payload)?))
}

/// A server stream: each message, then the end-of-stream frame with the
/// error that ended it, if any.
pub fn stream_response<T, S>(codec: Codec, messages: S) -> Response
where
    T: prost::Message + Serialize + Send + 'static,
    S: Stream<Item = RpcResult<T>> + Send + 'static,
{
    use futures::StreamExt;
    let frames = async_stream_frames(codec, messages).map(Ok::<_, std::convert::Infallible>);
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static(codec.content_type(true)),
        )],
        Body::from_stream(frames),
    )
        .into_response()
}

fn async_stream_frames<T, S>(codec: Codec, messages: S) -> impl Stream<Item = Bytes> + Send
where
    T: prost::Message + Serialize + Send + 'static,
    S: Stream<Item = RpcResult<T>> + Send + 'static,
{
    use futures::StreamExt;
    let messages = Box::pin(messages);
    futures::stream::unfold(Some(messages), move |state| async move {
        let mut messages = state?;
        match messages.next().await {
            Some(Ok(m)) => match codec.encode(&m) {
                Ok(b) => Some((envelope(0, &b), Some(messages))),
                Err(e) => Some((end_frame(Some(&e)), None)),
            },
            Some(Err(e)) => Some((end_frame(Some(&e)), None)),
            None => Some((end_frame(None), None)),
        }
    })
}

fn end_frame(err: Option<&RpcError>) -> Bytes {
    let body = match err {
        Some(e) => json!({"error": e.wire()}),
        None => json!({}),
    };
    envelope(0x02, &serde_json::to_vec(&body).unwrap_or_default())
}
