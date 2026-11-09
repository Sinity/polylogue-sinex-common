//! Which revisions of an authored record are current: a pure function of the
//! `revises` and `withdraws` links, never of write order or clocks.
//!
//! Revision R is **withdrawn** while a withdrawal that is not itself withdrawn
//! names R or R's root. R is **current** when it is not withdrawn and no
//! revision reachable from R through incoming `revises` links (its
//! descendants, transitively) is not withdrawn. Withdrawn revisions are thus
//! transparent: in R1←R2←R3, withdrawing R2 leaves R3 current and R1 not.
//! Two or more current revisions of one root are a conflict.
use crate::record::{Header, Revision, Target};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

/// The link lookups currency needs. A product implements them over its own
/// indexes (Polylogue's `link` table, Sinex's curation records); [`History`]
/// implements them in memory.
///
/// [`History`]: crate::History
pub trait Links {
    type Error;
    /// Every revision of the record with this root, the first included.
    fn revisions(&mut self, root: &str) -> Result<Vec<Revision>, Self::Error>;
    /// The revisions whose `revises` names this revision.
    fn revisers(&mut self, revision: &Revision) -> Result<Vec<Revision>, Self::Error>;
    /// The records whose `withdraws` names exactly this target.
    fn withdrawers(&mut self, target: &Target) -> Result<Vec<Revision>, Self::Error>;
    /// What this record's `withdraws` names.
    fn withdraws(&mut self, record: &Revision) -> Result<Vec<Target>, Self::Error>;
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CurrencyError<E> {
    #[error("reading links failed")]
    Source(E),
    /// Withdrawals that withdraw each other have no meaning; currency reports
    /// the cycle instead of letting evaluation order choose a winner.
    #[error("withdrawals of {0} form a cycle")]
    Cycle(Revision),
}

/// The state of one revision.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RevisionState {
    pub revision: Revision,
    pub withdrawn: bool,
    pub current: bool,
}

/// The state of every revision of one record, in revision order. This is what
/// a product's stored currency index holds for the root.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RootState {
    pub root: String,
    pub revisions: Vec<RevisionState>,
}

impl RootState {
    pub fn heads(&self) -> Vec<Revision> {
        self.revisions
            .iter()
            .filter(|state| state.current)
            .map(|state| state.revision.clone())
            .collect()
    }

    pub fn conflict(&self) -> bool {
        self.revisions.iter().filter(|state| state.current).count() > 1
    }
}

/// Evaluates currency over one consistent view of the links, remembering
/// withdrawal states it has already computed. Make a new one after the links
/// change.
pub struct Currency<'a, L: Links> {
    links: &'a mut L,
    withdrawn: HashMap<Revision, bool>,
}

impl<'a, L: Links> Currency<'a, L> {
    pub fn new(links: &'a mut L) -> Self {
        Self {
            links,
            withdrawn: HashMap::new(),
        }
    }

    /// Whether a withdrawal that is not itself withdrawn names this revision
    /// or its root. Iterative, so a long chain of withdrawals of withdrawals
    /// has no depth limit.
    pub fn withdrawn(&mut self, revision: &Revision) -> Result<bool, CurrencyError<L::Error>> {
        enum Step {
            Enter(Revision),
            Finish(Revision, Vec<Revision>),
        }
        if let Some(known) = self.withdrawn.get(revision) {
            return Ok(*known);
        }
        let mut on_path = HashSet::new();
        let mut steps = vec![Step::Enter(revision.clone())];
        while let Some(step) = steps.pop() {
            match step {
                Step::Enter(revision) => {
                    if self.withdrawn.contains_key(&revision) {
                        continue;
                    }
                    if !on_path.insert(revision.clone()) {
                        return Err(CurrencyError::Cycle(revision));
                    }
                    let mut withdrawers = self
                        .links
                        .withdrawers(&Target::Revision(revision.clone()))
                        .map_err(CurrencyError::Source)?;
                    withdrawers.extend(
                        self.links
                            .withdrawers(&Target::Root(revision.root.clone()))
                            .map_err(CurrencyError::Source)?,
                    );
                    withdrawers.sort();
                    withdrawers.dedup();
                    // Every withdrawer is entered, active or not, so a cycle is
                    // found whatever order the links arrive in.
                    let entered = withdrawers.clone();
                    steps.push(Step::Finish(revision, withdrawers));
                    steps.extend(entered.into_iter().rev().map(Step::Enter));
                }
                Step::Finish(revision, withdrawers) => {
                    let active = withdrawers
                        .iter()
                        .any(|withdrawal| !self.withdrawn[withdrawal]);
                    on_path.remove(&revision);
                    self.withdrawn.insert(revision, active);
                }
            }
        }
        Ok(self.withdrawn[revision])
    }

    /// Whether the revision is current: not withdrawn, and no descendant
    /// through `revises` is not withdrawn.
    pub fn current(&mut self, revision: &Revision) -> Result<bool, CurrencyError<L::Error>> {
        if self.withdrawn(revision)? {
            return Ok(false);
        }
        let mut seen = HashSet::from([revision.clone()]);
        let mut queue: VecDeque<Revision> = self
            .links
            .revisers(revision)
            .map_err(CurrencyError::Source)?
            .into();
        while let Some(descendant) = queue.pop_front() {
            if !seen.insert(descendant.clone()) {
                continue;
            }
            if !self.withdrawn(&descendant)? {
                return Ok(false);
            }
            queue.extend(
                self.links
                    .revisers(&descendant)
                    .map_err(CurrencyError::Source)?,
            );
        }
        Ok(true)
    }

    /// Every current revision of the root, sorted. More than one is a
    /// conflict; no head is chosen by time.
    pub fn heads(&mut self, root: &str) -> Result<Vec<Revision>, CurrencyError<L::Error>> {
        Ok(self.root_state(root)?.heads())
    }

    pub fn root_state(&mut self, root: &str) -> Result<RootState, CurrencyError<L::Error>> {
        let mut revisions = self.links.revisions(root).map_err(CurrencyError::Source)?;
        revisions.sort();
        revisions.dedup();
        let mut states = Vec::with_capacity(revisions.len());
        for revision in revisions {
            states.push(RevisionState {
                withdrawn: self.withdrawn(&revision)?,
                current: self.current(&revision)?,
                revision,
            });
        }
        Ok(RootState {
            root: root.to_owned(),
            revisions: states,
        })
    }
}

/// The roots whose currency can change when `record` is added: its own root,
/// the roots of the revisions it revises (another record's, when it resolves
/// a conflict between records holding one slot), the roots it withdraws, and,
/// through withdrawals of withdrawals, the roots those withdrawals name, as
/// far as `withdraws` links reach. A product that
/// stores currency recomputes exactly these roots in the writing transaction.
pub fn affected_roots<L: Links>(
    links: &mut L,
    record: &Header,
) -> Result<BTreeSet<String>, L::Error> {
    let mut roots = BTreeSet::from([record.root.clone()]);
    roots.extend(record.revises.iter().map(|revision| revision.root.clone()));
    let mut visited = HashSet::new();
    let mut queue: VecDeque<Target> = record.withdraws.iter().cloned().collect();
    while let Some(target) = queue.pop_front() {
        if !visited.insert(target.clone()) {
            continue;
        }
        roots.insert(target.root().to_owned());
        // A withdrawn record that is itself a withdrawal changes the state of
        // what it names.
        let withdrawals = match &target {
            Target::Revision(revision) => vec![revision.clone()],
            Target::Root(root) => links.revisions(root)?,
        };
        for withdrawal in withdrawals {
            queue.extend(links.withdraws(&withdrawal)?);
        }
    }
    Ok(roots)
}
