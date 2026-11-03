use dispatch_core::sandbox::{RedactingWriter, redact, sanitize};
use std::io::Write;

fn entries(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| (*s).to_owned()).collect()
}
#[test]
fn sanitize_drops_parent_secret() {
    let parent = entries(&[
        "PATH=/usr/bin:/bin",
        "HOME=/Users/alice",
        "USER=alice",
        "AWS_SECRET_ACCESS_KEY=secret",
        "OPENAI_API_KEY=secret",
        "DATABASE_PASSWORD=secret",
        "LC_ALL=en_US.UTF-8",
        "XDG_CONFIG_HOME=/Users/alice/.config",
        "DISPATCH_HOME=/Users/alice/.dispatch",
    ]);
    let out = sanitize(&parent, &[]);
    for entry in &out {
        let key = entry.split('=').next().unwrap();
        assert!(!key.contains("SECRET") && !key.contains("PASSWORD") && !key.contains("KEY"));
    }
    for expected in [
        "PATH=/usr/bin:/bin",
        "HOME=/Users/alice",
        "USER=alice",
        "LC_ALL=en_US.UTF-8",
        "XDG_CONFIG_HOME=/Users/alice/.config",
        "DISPATCH_HOME=/Users/alice/.dispatch",
    ] {
        assert!(out.contains(&expected.to_owned()));
    }
}
#[test]
fn sanitize_honors_dispatch_env_allow() {
    let parent = entries(&[
        "PATH=/usr/bin",
        "HOME=/Users/alice",
        "CUSTOM_TOKEN=secret-token-val",
        "ANOTHER_VAR=my-value",
        "AWS_SECRET_ACCESS_KEY=should-be-dropped",
        "DISPATCH_ENV_ALLOW=CUSTOM_TOKEN, ANOTHER_VAR",
    ]);
    let out = sanitize(&parent, &[]);
    assert!(out.contains(&"CUSTOM_TOKEN=secret-token-val".to_owned()));
    assert!(out.contains(&"ANOTHER_VAR=my-value".to_owned()));
    assert!(!out.iter().any(|s| s.starts_with("AWS_SECRET_ACCESS_KEY=")));
}
#[test]
fn redact_covers_all_patterns_and_preserves_common_text() {
    for (input, expected) in [
        (
            "export GITHUB_TOKEN=ghp_1234567890abcdef",
            "export GITHUB_TOKEN=[REDACTED]",
        ),
        (
            "export GH_TOKEN=\"ghp_1234567890abcdef\"",
            "export GH_TOKEN=[REDACTED]",
        ),
        (
            "CLIENT_SECRET=super_secret_val123",
            "CLIENT_SECRET=[REDACTED]",
        ),
        ("DB_PASSWORD='my_pass_word_456'", "DB_PASSWORD=[REDACTED]"),
        (
            "OPENAI_API_KEY=sk-proj-abcdef123456",
            "OPENAI_API_KEY=[REDACTED]",
        ),
        (
            "AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "AWS_SECRET_ACCESS_KEY=[REDACTED]",
        ),
        (
            "Authorization: Bearer my-access-token-123",
            "Authorization: [REDACTED]",
        ),
        (
            "Authorization: Basic dXNlcjpwYXNzd29yZA==",
            "Authorization: [REDACTED]",
        ),
        (
            "Failed with Bearer tok_9988776655 in response",
            "Failed with Bearer [REDACTED] in response",
        ),
        (
            "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA0m...\n-----END RSA PRIVATE KEY-----\nafter",
            "before\n[REDACTED]\nafter",
        ),
        (
            "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAA...\n-----END OPENSSH PRIVATE KEY-----",
            "[REDACTED]",
        ),
        (
            "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC...\n-----END PRIVATE KEY-----",
            "[REDACTED]",
        ),
        (
            "The quick brown fox jumps over the lazy dog.\nEverything passed successfully (0.42s).",
            "The quick brown fox jumps over the lazy dog.\nEverything passed successfully (0.42s).",
        ),
        (
            "User requested a new token from the administrator. Secret sharing is not permitted.",
            "User requested a new token from the administrator. Secret sharing is not permitted.",
        ),
        (
            "-----BEGIN CERTIFICATE-----\nMIICv...\n-----END CERTIFICATE-----",
            "-----BEGIN CERTIFICATE-----\nMIICv...\n-----END CERTIFICATE-----",
        ),
    ] {
        assert_eq!(redact(input), expected, "{input}");
    }
}
#[test]
fn redacting_writer() {
    let mut writer = RedactingWriter(Vec::new());
    let input = b"api_key=secret-val-123\nAuthorization: Bearer xyz\n";
    assert_eq!(writer.write(input).unwrap(), input.len());
    assert_eq!(writer.0, b"api_key=[REDACTED]\nAuthorization: [REDACTED]\n");
}
