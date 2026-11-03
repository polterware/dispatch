use crate::Error;

pub const BAND_MID: [f64; 3] = [-2.8, -1.8, -0.8];
#[derive(Clone, Debug, PartialEq)]
pub struct Difficulty {
    pub probs: Option<[f64; 3]>,
    pub tier: String,
    pub d: f64,
}
pub fn tier_from_probs(p: [f64; 3]) -> &'static str {
    if p[2] >= 0.2 {
        "hard"
    } else if p[1] + p[2] >= 0.35 {
        "normal"
    } else {
        "easy"
    }
}
pub fn new_difficulty(probs: Option<[f64; 3]>, tier: &str) -> Result<Difficulty, Error> {
    if let Some(mut p) = probs {
        for v in p {
            if !v.is_finite() || v < 0.0 {
                return Err(Error::Message(format!("classifier: bad probability {v}")));
            }
        }
        let sum: f64 = p.iter().sum();
        if sum <= 0.0 {
            return Err(Error::Message("classifier: probabilities sum to 0".into()));
        }
        for v in &mut p {
            *v /= sum;
        }
        let tier = tier_from_probs(p);
        let d: f64 = p.iter().zip(BAND_MID).map(|(a, b)| a * b).sum();
        let (lo, hi) = match tier {
            "easy" => (-4.0, -2.300001),
            "normal" => (-2.3, -1.300001),
            _ => (-1.3, 1.0),
        };
        Ok(Difficulty {
            probs: Some(p),
            tier: tier.into(),
            d: d.clamp(lo, hi),
        })
    } else {
        let i = match tier {
            "easy" => 0,
            "normal" => 1,
            "hard" => 2,
            _ => return Err(Error::Message(format!("classifier: unknown tier {tier:?}"))),
        };
        Ok(Difficulty {
            probs: None,
            tier: tier.into(),
            d: BAND_MID[i],
        })
    }
}
