use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Area {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tech: Vec<String>,
}

impl Area {
    pub fn new(kind: &str, tech: &str) -> Self {
        fn tokens(value: &str) -> Vec<String> {
            let mut out = Vec::new();
            for part in value.split(',') {
                let token = part.trim().to_lowercase();
                if !token.is_empty() && !out.contains(&token) {
                    out.push(token);
                }
            }
            out
        }
        Self {
            kinds: tokens(kind),
            tech: tokens(tech),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty() && self.tech.is_empty()
    }
}

pub fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z.clamp(-30.0, 30.0)).exp())
}
pub fn pass_probability(theta: f64, d: f64, uncertainty: f64) -> (f64, f64) {
    (sigmoid(theta - d), sigmoid(theta - d - uncertainty))
}
pub fn theta_uncertainty(source: &str, extrapolated_levels: usize) -> f64 {
    let base = if source == "unknown" {
        0.8
    } else if source.starts_with("eci") {
        0.5
    } else {
        0.3
    };
    base + 0.1 * extrapolated_levels as f64
}
pub fn difficulty_tier(d: f64) -> &'static str {
    if d < -2.3 {
        "easy"
    } else if d < -1.3 {
        "normal"
    } else {
        "hard"
    }
}
