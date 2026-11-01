//! `follow`, `wait` and `backtest`: the evaluator used without recording.
use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The evaluator delivered to an open connection. It records nothing, and it
/// has no baseline: rows that already match are its first messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Follow {
    evaluator: Evaluator,
}
impl Follow {
    pub fn new(
        rule: ActiveRule,
        start: Instant,
        calendar: &impl Calendar,
    ) -> Result<Self, EvaluationError> {
        let mut evaluator = Evaluator::new(start, vec![rule], calendar)?;
        evaluator.without_baseline();
        Ok(Self { evaluator })
    }
    pub fn observe(&mut self, evaluation: Evaluation) -> Result<(), EvaluationError> {
        self.evaluator.observe(evaluation)
    }
    pub fn next_due(&self) -> Option<Instant> {
        self.evaluator.next_due()
    }
    /// The messages up to `stop`: each firing, at the instant it fired.
    pub fn advance(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
    ) -> Result<Vec<Fired>, EvaluationError> {
        Ok(self
            .evaluator
            .reports(stop, calendar)?
            .into_iter()
            .filter_map(|report| match report {
                Report::Fired(fired) => Some(*fired),
                _ => None,
            })
            .collect())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitUntil {
    /// The first message the same follow would emit.
    #[default]
    Rows,
    /// The first complete evaluation with no rows.
    Empty,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "end", rename_all = "snake_case")]
pub enum WaitEnd {
    Fired {
        fired: Box<Fired>,
    },
    Empty {
        at: Instant,
    },
    /// The CLI exits 7; the answer carries the last evaluation.
    Timeout {
        at: Instant,
        last: Option<Box<Evaluation>>,
    },
}

/// `QUERY wait`: follow with an exit. It returns at the instant follow with
/// the same arguments would emit its first message, and records nothing but
/// its run. `timeout` is required: blocking has a bound.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Wait {
    follow: Follow,
    until: WaitUntil,
    deadline: Instant,
    last: Option<Evaluation>,
    queued: Vec<Evaluation>,
}
impl Wait {
    pub fn new(
        rule: ActiveRule,
        until: WaitUntil,
        start: Instant,
        timeout: Duration,
        calendar: &impl Calendar,
    ) -> Result<Self, EvaluationError> {
        Ok(Self {
            follow: Follow::new(rule, start, calendar)?,
            until,
            deadline: start.saturating_add(timeout.0),
            last: None,
            queued: Vec::new(),
        })
    }
    pub fn observe(&mut self, evaluation: Evaluation) -> Result<(), EvaluationError> {
        self.follow.observe(evaluation.clone())?;
        let index = self
            .queued
            .partition_point(|queued| queued.at <= evaluation.at);
        self.queued.insert(index, evaluation);
        Ok(())
    }
    /// The next instant at which the wait could end without a new evaluation.
    pub fn next_due(&self) -> Instant {
        self.follow
            .next_due()
            .map_or(self.deadline, |due| due.min(self.deadline))
    }
    pub fn advance(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
    ) -> Result<Option<WaitEnd>, EvaluationError> {
        let stop = stop.min(self.deadline);
        let mut fired = self.follow.advance(stop, calendar)?;
        let seen = self
            .queued
            .partition_point(|evaluation| evaluation.at <= stop);
        let evaluations: Vec<_> = self.queued.drain(..seen).collect();
        let empty = evaluations
            .iter()
            .find(|evaluation| evaluation.complete && evaluation.rows.is_empty())
            .map(|evaluation| evaluation.at);
        if let Some(evaluation) = evaluations.last() {
            self.last = Some(evaluation.clone());
        }
        let end = match self.until {
            WaitUntil::Rows => (!fired.is_empty()).then(|| WaitEnd::Fired {
                fired: Box::new(fired.remove(0)),
            }),
            WaitUntil::Empty => empty.map(|at| WaitEnd::Empty { at }),
        };
        Ok(end.or_else(|| {
            (stop >= self.deadline).then(|| WaitEnd::Timeout {
                at: self.deadline,
                last: self.last.clone().map(Box::new),
            })
        }))
    }
}

/// Everything a backtest replays: the rules as they were, the stored
/// observations, and the attention facts of the interval.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct History {
    pub observations: Vec<Evaluation>,
    pub attention: Vec<(Instant, AttentionInput)>,
}

/// Replay rules over stored history: the deliveries they would have made in
/// `[start, stop]`, each with the route decision it would have received under
/// the attention of that time. It writes nothing.
pub fn backtest(
    rules: Vec<ActiveRule>,
    history: &History,
    start: Instant,
    stop: Instant,
    settings: HostSettings,
    budget_per_hour: usize,
    calendar: &impl Calendar,
) -> Result<Handoffs, EvaluationError> {
    let mut evaluator = Evaluator::new(start, rules, calendar)?;
    let mut host = HostPolicy::new(
        settings,
        InterruptBudget {
            per_hour: budget_per_hour,
            handoffs: Vec::new(),
        },
    );
    for evaluation in history
        .observations
        .iter()
        .filter(|evaluation| evaluation.at >= start && evaluation.at <= stop)
    {
        evaluator.observe(evaluation.clone())?;
    }
    for (at, input) in history
        .attention
        .iter()
        .filter(|(at, _)| *at >= start && *at <= stop)
    {
        host.observe(*at, input.clone())?;
    }
    evaluator.advance(stop, calendar, &mut host)
}
