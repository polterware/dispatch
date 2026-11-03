use regex::Regex;
use std::io::{self, Write};
use std::sync::{Arc, LazyLock, Mutex};
use tokio_util::sync::CancellationToken;

static CREDIT: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)ActionRequiredError|not included in your (?:\w+ )?(?:usage|plan|subscription)|buy (?:more )?credits|purchase (?:more )?credits|payment required|\b402\b|upgrade (?:your )?(?:plan|subscription) to (?:use|continue|access)").ok()
});
pub fn is_credit_error(s: &str) -> bool {
    CREDIT.as_ref().is_some_and(|re| re.is_match(s))
}
#[derive(Clone)]
pub struct FatalWatch {
    state: Arc<Mutex<State>>,
    cancel: CancellationToken,
}
#[derive(Default)]
struct State {
    line: Vec<u8>,
    hit: String,
}
impl FatalWatch {
    pub fn new(cancel: CancellationToken) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            cancel,
        }
    }
    pub fn fatal(&self) -> String {
        self.state
            .lock()
            .ok()
            .map(|s| s.hit.clone())
            .unwrap_or_default()
    }
}
impl Write for FatalWatch {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Ok(mut state) = self.state.lock() else {
            return Ok(buf.len());
        };
        if !state.hit.is_empty() {
            return Ok(buf.len());
        }
        state.line.extend_from_slice(buf);
        while let Some(i) = state.line.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = state.line.drain(..=i).collect();
            let value = String::from_utf8_lossy(&line).trim().to_owned();
            if is_credit_error(&value) {
                state.hit = value;
                state.line.clear();
                self.cancel.cancel();
                return Ok(buf.len());
            }
        }
        let value = String::from_utf8_lossy(&state.line).trim().to_owned();
        if is_credit_error(&value) {
            state.hit = value;
            state.line.clear();
            self.cancel.cancel();
        } else if state.line.len() > 64 << 10 {
            state.line.clear();
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
