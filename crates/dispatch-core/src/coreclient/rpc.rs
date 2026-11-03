use crate::Error;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::Duration;

/// Header naming the calling client (`desktop`, `cli`, `polterware`).
pub const CLIENT_HEADER: &str = "Pwdispatch-Client";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub struct ConnectError {
    pub code: String,
    pub message: String,
    pub transport: bool,
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code.is_empty(), self.message.is_empty()) {
            (true, _) => f.write_str(&self.message),
            (false, true) => f.write_str(&self.code),
            (false, false) => write!(f, "{}: {}", self.code, self.message),
        }
    }
}

impl From<ConnectError> for Error {
    fn from(err: ConnectError) -> Self {
        Error::Message(err.to_string())
    }
}

#[derive(Deserialize)]
struct ConnectErrorWire {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct EndStreamWire {
    #[serde(default)]
    error: Option<ConnectErrorWire>,
}

pub fn http_status_to_connect_code(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "invalid_argument",
        401 => "unauthenticated",
        403 => "permission_denied",
        404 => "unimplemented",
        408 => "deadline_exceeded",
        409 => "aborted",
        412 => "failed_precondition",
        413 | 429 => "resource_exhausted",
        415 => "internal",
        499 => "canceled",
        502..=504 => "unavailable",
        _ => "unknown",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub flags: u8,
    pub payload: Vec<u8>,
}

pub fn encode_envelope(flags: u8, payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(5 + len as usize);
    out.push(flags);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&payload[..len as usize]);
    out
}

#[derive(Clone, Debug, Default)]
pub struct EnvelopeDecoder {
    buf: Vec<u8>,
}

impl EnvelopeDecoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn pending_bytes(&self) -> usize {
        self.buf.len()
    }

    pub fn next_frame(&mut self) -> Option<Envelope> {
        if self.buf.len() < 5 {
            return None;
        }
        let flags = self.buf[0];
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if self.buf.len() < 5 + len {
            return None;
        }
        let payload = self.buf[5..5 + len].to_vec();
        self.buf.drain(..5 + len);
        Some(Envelope { flags, payload })
    }
}

pub fn decode_stream_frame<T: DeserializeOwned>(
    frame: &Envelope,
) -> Result<Option<T>, ConnectError> {
    if frame.flags & 0x01 != 0 {
        return Err(ConnectError {
            code: "internal".into(),
            message: "compressed connect frames are not supported".into(),
            transport: false,
        });
    }
    if frame.flags & 0x02 != 0 {
        if frame.payload.is_empty() {
            return Ok(None);
        }
        let parsed: EndStreamWire =
            serde_json::from_slice(&frame.payload).map_err(|e| ConnectError {
                code: "internal".into(),
                message: format!("invalid end-of-stream message: {e}"),
                transport: false,
            })?;
        if let Some(err) = parsed.error {
            return Err(ConnectError {
                code: if err.code.is_empty() {
                    "unknown".into()
                } else {
                    err.code
                },
                message: err.message,
                transport: false,
            });
        }
        return Ok(None);
    }
    serde_json::from_slice(&frame.payload)
        .map(Some)
        .map_err(|e| ConnectError {
            code: "internal".into(),
            message: e.to_string(),
            transport: false,
        })
}

pub(crate) fn parse_error_response(status: StatusCode, body: &[u8]) -> ConnectError {
    if let Ok(parsed) = serde_json::from_slice::<ConnectErrorWire>(body)
        && (!parsed.code.is_empty() || !parsed.message.is_empty())
    {
        return ConnectError {
            code: if parsed.code.is_empty() {
                http_status_to_connect_code(status).into()
            } else {
                parsed.code
            },
            message: parsed.message,
            transport: false,
        };
    }
    ConnectError {
        code: http_status_to_connect_code(status).into(),
        message: format!("HTTP {}", status.as_u16()),
        transport: false,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RpcTransport {
    http: reqwest::Client,
    base: String,
    token: String,
    client_header_name: &'static str,
    client_header_value: String,
}

impl RpcTransport {
    pub(crate) fn new(
        endpoint: &str,
        token: &str,
        client_header_name: &'static str,
        client_header_value: &str,
        timeout: Option<Duration>,
    ) -> Self {
        let mut builder = reqwest::Client::builder().no_proxy();
        if let Some(t) = timeout {
            builder = builder.timeout(t);
        }
        let http = builder.build().unwrap_or_else(|_| reqwest::Client::new());
        Self {
            http,
            base: endpoint.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            client_header_name,
            client_header_value: client_header_value.to_owned(),
        }
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.base
    }

    pub(crate) async fn unary<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        procedure: &str,
        request: &Req,
    ) -> Result<Resp, ConnectError> {
        let url = format!("{}/{}", self.base, procedure.trim_start_matches('/'));
        let payload = serde_json::to_vec(request).map_err(|e| ConnectError {
            code: "internal".into(),
            message: e.to_string(),
            transport: false,
        })?;
        let mut builder = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .header("Authorization", format!("Bearer {}", self.token));
        if !self.client_header_value.is_empty() {
            builder = builder.header(self.client_header_name, &self.client_header_value);
        }
        let response = builder
            .body(payload)
            .send()
            .await
            .map_err(|e| ConnectError {
                code: "unavailable".into(),
                message: e.to_string(),
                transport: true,
            })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|e| ConnectError {
            code: "unavailable".into(),
            message: e.to_string(),
            transport: true,
        })?;
        if !status.is_success() {
            return Err(parse_error_response(status, &bytes));
        }
        let raw: &[u8] = if bytes.is_empty() { b"{}" } else { &bytes };
        serde_json::from_slice(raw).map_err(|e| ConnectError {
            code: "internal".into(),
            message: e.to_string(),
            transport: false,
        })
    }

    pub(crate) async fn open_stream<Req: Serialize>(
        &self,
        procedure: &str,
        request: &Req,
    ) -> Result<reqwest::Response, ConnectError> {
        let url = format!("{}/{}", self.base, procedure.trim_start_matches('/'));
        let payload = serde_json::to_vec(request).map_err(|e| ConnectError {
            code: "internal".into(),
            message: e.to_string(),
            transport: false,
        })?;
        let framed = encode_envelope(0, &payload);
        let mut builder = self
            .http
            .post(&url)
            .header("Content-Type", "application/connect+json")
            .header("Connect-Protocol-Version", "1")
            .header("Authorization", format!("Bearer {}", self.token));
        if !self.client_header_value.is_empty() {
            builder = builder.header(self.client_header_name, &self.client_header_value);
        }
        let response = builder
            .body(framed)
            .send()
            .await
            .map_err(|e| ConnectError {
                code: "unavailable".into(),
                message: e.to_string(),
                transport: true,
            })?;
        let status = response.status();
        if !status.is_success() {
            let bytes = response.bytes().await.unwrap_or_default();
            return Err(parse_error_response(status, &bytes));
        }
        Ok(response)
    }
}
