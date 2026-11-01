//! A product's status: figures in the shared value states, and one verdict.
use crate::{OperationFuture, ValueState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// One value in a product's status: its name, its value with its state, and
/// how old the measurement is in milliseconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Figure<T> {
    name: String,
    value: ValueState<T>,
    age: u64,
}
impl<T> Figure<T> {
    pub fn new(name: impl Into<String>, value: ValueState<T>) -> Self {
        Self {
            name: name.into(),
            value,
            age: 0,
        }
    }
    pub fn measured(name: impl Into<String>, value: T) -> Self {
        Self::new(name, ValueState::Complete { value })
    }
    pub fn not_measured(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::new(
            name,
            ValueState::Unknown {
                reason: reason.into(),
            },
        )
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn value(&self) -> &ValueState<T> {
        &self.value
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
    /// Healthy when every figure is sound; unknown when no figure holds a
    /// value; degraded otherwise, naming the first figure that is not sound.
    pub fn from_figures<T>(figures: &[Figure<T>]) -> Self {
        if figures.is_empty() {
            return Self::Unknown {
                reason: "no figures measured".into(),
            };
        }
        let Some(first) = figures.iter().find(|f| !f.value.is_sound()) else {
            return Self::Healthy;
        };
        let reason = format!(
            "{}: {}",
            first.name,
            first.value.reason().unwrap_or(first.value.name())
        );
        if figures
            .iter()
            .all(|f| !f.value.is_sound() && f.value.value().is_none())
        {
            Self::Unknown { reason }
        } else {
            Self::Degraded { reason }
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
