use crate::{
    Error,
    adapters::Request,
    config::CLASSIFIER_CLI,
    difficulty::{DIFFICULTY_LEVELS, DIFFICULTY_QUESTION, ROLE_QUESTION, new_difficulty},
    runner::{AgentAdapter, AgentCall, Classification, Classifier, RoleAnswer, Task},
};
use futures::future::BoxFuture;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Executes one read-only prompt and returns the agent's reply.
pub type RunFn = Arc<dyn Fn(String) -> BoxFuture<'static, Result<String, Error>> + Send + Sync>;

/// Asks a cheap model through an installed agent CLI to name the tier. It
/// spends the person's quota and a few seconds per run, so it is opt-in.
#[derive(Clone, Default)]
pub struct Cli {
    pub agent: String,
    pub model: String,
    pub run: Option<RunFn>,
}

impl std::fmt::Debug for Cli {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cli")
            .field("agent", &self.agent)
            .field("model", &self.model)
            .finish()
    }
}

impl Cli {
    /// Runs `model` through `a`, read-only in an empty temporary folder, with
    /// a short timeout.
    pub fn with_adapter(a: Arc<dyn AgentAdapter>, model: String, timeout: Duration) -> Self {
        let agent = a.name();
        let run_model = model.clone();
        let run: RunFn = Arc::new(move |prompt: String| {
            let a = a.clone();
            let model = run_model.clone();
            Box::pin(async move {
                let dir = tempfile::Builder::new()
                    .prefix("dispatch-classify-")
                    .tempdir()?;
                let call = AgentCall {
                    request: Request {
                        prompt,
                        tier: "easy".into(),
                        dir: dir.path().to_string_lossy().into_owned(),
                        read_only: true,
                        timeout,
                        ..Request::default()
                    },
                    model,
                    cancel: CancellationToken::new(),
                    started: Arc::new(|_| Ok(())),
                };
                let res = a.run(call).await;
                if let Some(e) = res.error {
                    return Err(Error::Message(e.to_string()));
                }
                if res.result.exit_code != 0 {
                    return Err(Error::Message(format!(
                        "{} exited {}",
                        a.name(),
                        res.result.exit_code
                    )));
                }
                Ok(res.result.output)
            })
        });
        Self {
            agent,
            model,
            run: Some(run),
        }
    }

    async fn ask(&self, t: Task) -> Result<Classification, Error> {
        let run = self
            .run
            .as_ref()
            .ok_or_else(|| Error::Message("cli classifier: no agent in the easy tier".into()))?;
        let start = Instant::now();
        let out = run(cli_prompt(&t.text, t.roles.as_ref())).await?;
        let tier = parse_tier(&out)?;
        let difficulty = new_difficulty(None, &tier)?;
        let role = parse_role(&out)
            .filter(|_| t.roles.as_ref().is_some_and(|r| !r.is_empty()))
            .map(|role| RoleAnswer {
                role,
                ..RoleAnswer::default()
            });
        Ok(Classification {
            difficulty,
            provider: CLASSIFIER_CLI.into(),
            model: format!("{}/{}", self.agent, self.model)
                .trim_matches('/')
                .to_owned(),
            confidence: None,
            latency: start.elapsed(),
            role,
        })
    }
}

impl Classifier for Cli {
    fn name(&self) -> String {
        CLASSIFIER_CLI.into()
    }

    fn classify(&self, task: Task) -> BoxFuture<'_, Result<Classification, Error>> {
        Box::pin(self.ask(task))
    }
}

/// The whole instruction the agent receives, the task included. With roles
/// it also asks which one fits, by name.
pub fn cli_prompt(task: &str, roles: Option<&BTreeMap<String, String>>) -> String {
    let mut b = String::from(
        "Classify a coding task. Judge from the text alone: do not read files, run commands, or start the work.\n\n",
    );
    b.push_str(DIFFICULTY_QUESTION);
    b.push('\n');
    for l in DIFFICULTY_LEVELS {
        b.push_str("- ");
        b.push_str(l);
        b.push('\n');
    }
    match roles.filter(|r| !r.is_empty()) {
        None => {
            b.push_str(
                "\nReply with only JSON like {\"tier\": \"<easy, normal, or hard>\"}.\n\nTask:\n",
            );
        }
        Some(roles) => {
            b.push('\n');
            b.push_str(ROLE_QUESTION);
            b.push('\n');
            for (name, when) in roles {
                b.push_str(&format!("- {name}: {when}\n"));
            }
            b.push_str("\nReply with only JSON like {\"tier\": \"<easy, normal, or hard>\", \"role\": \"<one role name>\"}.\n\nTask:\n");
        }
    }
    b.push_str(task);
    b
}

// Both also match the escaped quotes of a JSON event stream.
static ROLE_JSON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\\?"role\\?"\s*:\s*\\?"([a-z][a-z0-9-]*)\\?""#).expect("role pattern")
});
static TIER_JSON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\\?"tier\\?"\s*:\s*\\?"(easy|normal|hard)\\?""#).expect("tier pattern")
});
static TIER_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(easy|normal|hard)\b").expect("tier word pattern"));

/// The last {"role": …} in an agent's reply, None when there is none.
pub fn parse_role(out: &str) -> Option<String> {
    let low = out.to_lowercase();
    ROLE_JSON
        .captures_iter(&low)
        .last()
        .map(|c| c[1].to_owned())
}

/// The tier in an agent's reply: the last {"tier": …}, else the reply when it
/// is a single tier word.
pub fn parse_tier(out: &str) -> Result<String, Error> {
    let low = out.to_lowercase();
    if let Some(c) = TIER_JSON.captures_iter(&low).last() {
        return Ok(c[1].to_owned());
    }
    let words: Vec<&str> = TIER_WORD.find_iter(&low).map(|m| m.as_str()).collect();
    if let [word] = words[..] {
        return Ok(word.to_owned());
    }
    Err(Error::Message(
        "cli classifier: no tier in the reply".into(),
    ))
}
