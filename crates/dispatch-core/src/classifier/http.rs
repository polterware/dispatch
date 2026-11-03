use crate::Error;
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;

/// Bounds one HTTP attempt; retries add backoff on top.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How many times a rate-limited or overloaded call is tried again.
const RETRIES: u32 = 3;

/// Sends `body` with a bearer key and decodes the JSON answer, retrying 429,
/// 529, and 503 with exponential backoff.
pub(super) async fn post_json<B: Serialize, T: DeserializeOwned>(
    client: Option<&reqwest::Client>,
    url: &str,
    key: &str,
    body: &B,
) -> Result<T, Error> {
    let body = serde_json::to_vec(body)?;
    let default;
    let client = match client {
        Some(c) => c,
        None => {
            default = reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|e| Error::Message(e.to_string()))?;
            &default
        }
    };
    let mut backoff = Duration::from_millis(500);
    let mut attempt = 0;
    loop {
        let resp = client
            .post(url)
            .bearer_auth(key)
            .header("Content-Type", "application/json")
            .body(body.clone())
            .send()
            .await
            .map_err(|e| Error::Message(e.to_string()))?;
        let status = resp.status().as_u16();
        let data = resp
            .bytes()
            .await
            .map_err(|e| Error::Message(e.to_string()))?;
        let data = &data[..data.len().min(1 << 20)];
        match status {
            200 => {
                return serde_json::from_slice(data)
                    .map_err(|e| Error::Message(format!("bad response: {e}")));
            }
            429 | 529 | 503 if attempt < RETRIES => {
                tokio::time::sleep(backoff).await;
                backoff *= 2;
                attempt += 1;
            }
            _ => return Err(Error::Message(format!("HTTP {status}: {}", snippet(data)))),
        }
    }
}

fn snippet(b: &[u8]) -> String {
    let b = &b[..b.len().min(200)];
    String::from_utf8_lossy(b).trim().to_owned()
}
