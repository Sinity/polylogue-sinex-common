//! Status measurements keep measured, missing and unavailable apart; one
//! verdict is derived from them.
use crate::{Failure, OperationFuture, bad_request};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementState {
    Measured,
    NoneFound,
    NotMeasured,
    Unavailable,
    Gap,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "MeasurementWire<T>")]
pub struct Measurement<T> {
    name: String,
    value: Option<T>,
    state: MeasurementState,
    reason: Option<String>,
    age: u64,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MeasurementWire<T> {
    name: String,
    value: Option<T>,
    state: MeasurementState,
    reason: Option<String>,
    age: u64,
}
impl<T> TryFrom<MeasurementWire<T>> for Measurement<T> {
    type Error = Failure;
    fn try_from(w: MeasurementWire<T>) -> Result<Self, Failure> {
        Self::new(w.name, w.value, w.state, w.reason, w.age)
    }
}
impl<T> Measurement<T> {
    pub fn new(
        name: impl Into<String>,
        value: Option<T>,
        state: MeasurementState,
        reason: Option<String>,
        age: u64,
    ) -> Result<Self, Failure> {
        let measured = matches!(
            state,
            MeasurementState::Measured | MeasurementState::NoneFound
        );
        if measured != value.is_some() {
            return Err(bad_request(
                "only measured and none_found measurements carry values",
            ));
        }
        if !measured && reason.as_ref().is_none_or(|r| r.is_empty()) {
            return Err(bad_request("unmeasured measurements need a reason"));
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
            state: MeasurementState::Measured,
            reason: None,
            age: 0,
        }
    }
    pub fn not_measured(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: None,
            state: MeasurementState::NotMeasured,
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
    pub fn state(&self) -> MeasurementState {
        self.state
    }
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
    pub fn age(&self) -> u64 {
        self.age
    }
}
impl Measurement<f64> {
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
    pub fn from_measurements<T>(measurements: &[Measurement<T>]) -> Self {
        if measurements.is_empty() {
            return Self::Unknown {
                reason: "no measurements measured".into(),
            };
        }
        let missing = measurements.iter().find(|f| {
            !matches!(
                f.state,
                MeasurementState::Measured | MeasurementState::NoneFound
            )
        });
        match missing {
            None => Self::Healthy,
            Some(first) => {
                let reason = format!(
                    "{}: {}",
                    first.name,
                    first.reason.as_deref().unwrap_or("not measured")
                );
                if measurements.iter().all(|f| {
                    !matches!(
                        f.state,
                        MeasurementState::Measured | MeasurementState::NoneFound
                    )
                }) {
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
    pub measurements: Vec<Measurement<T>>,
    pub verdict: Verdict,
}
impl<T> Status<T> {
    pub fn new(measurements: Vec<Measurement<T>>) -> Self {
        let verdict = Verdict::from_measurements(&measurements);
        Self {
            measurements,
            verdict,
        }
    }
}

/// Caller computes a key from source positions or file/counter identities.
/// An unchanged key reuses the measurement; elapsed time only supplies age.
pub struct MeasurementCache<K, T> {
    cached: Option<(K, Measurement<T>, Instant)>,
}
impl<K, T> Default for MeasurementCache<K, T> {
    fn default() -> Self {
        Self { cached: None }
    }
}
impl<K: PartialEq, T: Clone> MeasurementCache<K, T> {
    pub fn get(&self, key: &K) -> Option<Measurement<T>> {
        self.cached
            .as_ref()
            .filter(|(k, _, _)| k == key)
            .map(|(_, measurement, at)| {
                let mut result = measurement.clone();
                result.age = u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX);
                result
            })
    }
    pub fn insert(&mut self, key: K, measurement: Measurement<T>) {
        self.cached = Some((key, measurement, Instant::now()));
    }
}
/// Run this future concurrently with the other measurements. Its own timeout
/// becomes a missing value while the remaining measurements complete normally.
pub fn measure<'a, T: Send + 'a>(
    name: impl Into<String>,
    deadline: Duration,
    measurement: OperationFuture<'a, Measurement<T>>,
) -> OperationFuture<'a, Measurement<T>> {
    let name = name.into();
    Box::pin(async move {
        match tokio::time::timeout(deadline, measurement).await {
            Ok(measurement) => measurement,
            Err(_) => Measurement::not_measured(name, "timed out"),
        }
    })
}
