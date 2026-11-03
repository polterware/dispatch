use dispatch_core::adapters::failfast::{FatalWatch, is_credit_error};
use std::io::Write;
use tokio_util::sync::CancellationToken;
#[test]
fn credit_re() {
    for s in [
        "ActionRequiredError: GPT-5.6 Sol is not included in your weekly usage Buy credits or switch to a Grok model to continue.",
        "This model is not included in your plan",
        "HTTP 402 Payment Required",
        "Please buy credits to continue",
    ] {
        assert!(is_credit_error(s), "{s}");
    }
    for s in [
        "429 too many requests",
        "credit card form validation",
        "port 14020 busy",
        "added credits to the ledger table",
    ] {
        assert!(!is_credit_error(s), "{s}");
    }
}
#[test]
fn fail_fast_watch() {
    let token = CancellationToken::new();
    let mut watch = FatalWatch::new(token.clone());
    watch.write_all(b"warming up\n").unwrap();
    assert!(!token.is_cancelled());
    assert!(watch.fatal().is_empty());
    watch
        .write_all(b"ActionRequiredError: not included in your weekly usage")
        .unwrap();
    assert!(token.is_cancelled());
    assert!(watch.fatal().starts_with("ActionRequiredError"));
}
#[test]
fn fail_fast_ignores_normal_exit() {
    let token = CancellationToken::new();
    let watch = FatalWatch::new(token.clone());
    token.cancel();
    assert!(token.is_cancelled());
    assert!(watch.fatal().is_empty());
}
