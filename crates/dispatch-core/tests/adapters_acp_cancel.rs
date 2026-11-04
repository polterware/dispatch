//! Cancelling an ACP run at any moment ends it as cancelled. macOS answers a signal to a
//! zombie (a subprocess the agent's shell has not reaped yet) with EPERM, which once
//! failed a cancelled run with "agent process cleanup incomplete".

use dispatch_core::adapters::{Request, acp, binaries};
use std::{collections::HashMap, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_cancel_at_any_moment_is_a_cancel() {
    let fake = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/e2e/fake-acp.sh"
    );
    binaries::set_binaries(HashMap::from([("opencode".into(), fake.into())]));
    let dir = tempfile::tempdir().unwrap();
    let mut errors = Vec::new();
    for i in 0..30u64 {
        let request = Request {
            dir: dir.path().to_string_lossy().into_owned(),
            prompt: "SLOW".into(),
            read_only: true,
            log_path: dir
                .path()
                .join(format!("log{i}"))
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        };
        let cancel = CancellationToken::new();
        let c = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20 + (i % 6) * 40)).await;
            c.cancel();
        });
        let r = acp::run("opencode", &request, "fake-fast", cancel).await;
        if let Err(e) = r {
            let m = e.to_string();
            if m != "context canceled" {
                errors.push(format!("{i}: {m}"));
            }
        }
    }
    assert!(errors.is_empty(), "{errors:#?}");
}
