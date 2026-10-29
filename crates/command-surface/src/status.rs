//! Status values preserve measured, missing and unavailable as distinct states.
use crate::{Failure, OperationFuture, bad_request};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FigureState {
    Measured,
    NoneFound,
    NotMeasured,
    Unavailable,
    Gap,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "FigureWire<T>")]
pub struct Figure<T> {
    name: String,
    value: Option<T>,
    state: FigureState,
    reason: Option<String>,
    age: u64,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FigureWire<T> {
    name: String,
    value: Option<T>,
    state: FigureState,
    reason: Option<String>,
    age: u64,
}
impl<T> TryFrom<FigureWire<T>> for Figure<T> {
    type Error = Failure;
    fn try_from(w: FigureWire<T>) -> Result<Self, Failure> {
        Self::new(w.name, w.value, w.state, w.reason, w.age)
    }
}
impl<T> Figure<T> {
    pub fn new(
        name: impl Into<String>,
        value: Option<T>,
        state: FigureState,
        reason: Option<String>,
        age: u64,
    ) -> Result<Self, Failure> {
        let measured = matches!(state, FigureState::Measured | FigureState::NoneFound);
        if measured != value.is_some() {
            return Err(bad_request(
                "only measured and none_found figures carry values",
            ));
        }
        if !measured && reason.as_ref().is_none_or(|r| r.is_empty()) {
            return Err(bad_request("unmeasured figures need a reason"));
        }
        Ok(Self {
            name: name.into(),
            value,
            state,
            reason,
            age,
        })
    }
    pub fn measured(name: impl Into<String>, value: T) -> Self {
        Self {
            name: name.into(),
            value: Some(value),
            state: FigureState::Measured,
            reason: None,
            age: 0,
        }
    }
    pub fn not_measured(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: None,
            state: FigureState::NotMeasured,
            reason: Some(reason.into()),
            age: 0,
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn value(&self) -> Option<&T> {
        self.value.as_ref()
    }
    pub fn state(&self) -> FigureState {
        self.state
    }
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
    pub fn age(&self) -> u64 {
        self.age
    }
}
impl Figure<f64> {
    pub fn ratio(name: impl Into<String>, numerator: u64, denominator: u64) -> Self {
        if denominator == 0 {
            Self::not_measured(name, "zero denominator")
        } else {
            Self::measured(name, numerator as f64 / denominator as f64)
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Verdict {
    Healthy,
    Degraded { reason: String },
    Unknown { reason: String },
}
impl Verdict {
    pub fn from_figures<T>(figures: &[Figure<T>]) -> Self {
        if figures.is_empty() {
            return Self::Unknown {
                reason: "no figures measured".into(),
            };
        }
        let missing = figures
            .iter()
            .find(|f| !matches!(f.state, FigureState::Measured | FigureState::NoneFound));
        match missing {
            None => Self::Healthy,
            Some(first) => {
                let reason = format!(
                    "{}: {}",
                    first.name,
                    first.reason.as_deref().unwrap_or("not measured")
                );
                if figures
                    .iter()
                    .all(|f| !matches!(f.state, FigureState::Measured | FigureState::NoneFound))
                {
                    Self::Unknown { reason }
                } else {
                    Self::Degraded { reason }
                }
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Status<T> {
    pub figures: Vec<Figure<T>>,
    pub verdict: Verdict,
}
impl<T> Status<T> {
    pub fn new(figures: Vec<Figure<T>>) -> Self {
        let verdict = Verdict::from_figures(&figures);
        Self { figures, verdict }
    }
}

/// Caller computes a key from source positions or file/counter identities.
/// An unchanged key reuses the measurement; elapsed time only supplies age.
pub struct FigureCache<K, T> {
    cached: Option<(K, Figure<T>, Instant)>,
}
impl<K, T> Default for FigureCache<K, T> {
    fn default() -> Self {
        Self { cached: None }
    }
}
impl<K: PartialEq, T: Clone> FigureCache<K, T> {
    pub fn get(&self, key: &K) -> Option<Figure<T>> {
        self.cached
            .as_ref()
            .filter(|(k, _, _)| k == key)
            .map(|(_, figure, at)| {
                let mut result = figure.clone();
                result.age = u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX);
                result
            })
    }
    pub fn insert(&mut self, key: K, figure: Figure<T>) {
        self.cached = Some((key, figure, Instant::now()));
    }
}
/// Run this future concurrently with the other figures. Its own timeout
/// becomes a missing value while the remaining figures complete normally.
pub fn measure_figure<'a, T: Send + 'a>(
    name: impl Into<String>,
    deadline: Duration,
    measurement: OperationFuture<'a, Figure<T>>,
) -> OperationFuture<'a, Figure<T>> {
    let name = name.into();
    Box::pin(async move {
        match tokio::time::timeout(deadline, measurement).await {
            Ok(figure) => figure,
            Err(_) => Figure::not_measured(name, "timed out"),
        }
    })
}
