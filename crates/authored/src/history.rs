//! Authored records held in memory, with the lookups currency and requests
//! need. Tests in both products use it as the reference store; a product may
//! also use it where its histories are small enough to hold.
use crate::currency::{Currency, CurrencyError, Links, RootState};
use crate::record::{Header, Revision, Target};
use crate::request::Planned;
use crate::uniqueness::{Holders, UniqueSlot};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::convert::Infallible;

#[derive(Debug, Clone, Default)]
pub struct History {
    records: Vec<Header>,
    by_revision: HashMap<Revision, usize>,
    by_root: BTreeMap<String, Vec<Revision>>,
    revisers: HashMap<Revision, Vec<Revision>>,
    withdrawers: HashMap<Target, Vec<Revision>>,
    holders: HashMap<UniqueSlot, Vec<Revision>>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one record with the slots its content holds. Adding a revision
    /// that is already present replaces nothing and returns false.
    pub fn push(&mut self, header: Header, slots: Vec<UniqueSlot>) -> bool {
        let revision = header.revision();
        if self.by_revision.contains_key(&revision) {
            return false;
        }
        self.by_revision
            .insert(revision.clone(), self.records.len());
        self.by_root
            .entry(revision.root.clone())
            .or_default()
            .push(revision.clone());
        for base in &header.revises {
            self.revisers
                .entry(base.clone())
                .or_default()
                .push(revision.clone());
        }
        for target in &header.withdraws {
            self.withdrawers
                .entry(target.clone())
                .or_default()
                .push(revision.clone());
        }
        for slot in slots {
            self.holders.entry(slot).or_default().push(revision.clone());
        }
        self.records.push(header);
        true
    }

    /// Adds every record of a placed request.
    pub fn commit<B>(&mut self, planned: Planned<B>) {
        for record in planned.records {
            self.push(record.header, record.slots);
        }
    }

    /// The records in the order they were added.
    pub fn records(&self) -> &[Header] {
        &self.records
    }

    pub fn record(&self, revision: &Revision) -> Option<&Header> {
        self.by_revision
            .get(revision)
            .map(|index| &self.records[*index])
    }

    pub fn roots(&self) -> impl Iterator<Item = &str> {
        self.by_root.keys().map(String::as_str)
    }

    /// The state of every root, in root order.
    pub fn states(&mut self) -> Result<Vec<RootState>, CurrencyError<Infallible>> {
        let roots: Vec<String> = self.by_root.keys().cloned().collect();
        let mut currency = Currency::new(self);
        roots.iter().map(|root| currency.root_state(root)).collect()
    }

    /// Every current revision across all roots.
    pub fn current(&mut self) -> Result<BTreeSet<Revision>, CurrencyError<Infallible>> {
        Ok(self.states()?.iter().flat_map(RootState::heads).collect())
    }
}

impl Links for History {
    type Error = Infallible;

    fn revisions(&mut self, root: &str) -> Result<Vec<Revision>, Infallible> {
        Ok(self.by_root.get(root).cloned().unwrap_or_default())
    }

    fn revisers(&mut self, revision: &Revision) -> Result<Vec<Revision>, Infallible> {
        Ok(self.revisers.get(revision).cloned().unwrap_or_default())
    }

    fn withdrawers(&mut self, target: &Target) -> Result<Vec<Revision>, Infallible> {
        Ok(self.withdrawers.get(target).cloned().unwrap_or_default())
    }

    fn withdraws(&mut self, record: &Revision) -> Result<Vec<Target>, Infallible> {
        Ok(self
            .record(record)
            .map(|header| header.withdraws.iter().cloned().collect())
            .unwrap_or_default())
    }
}

impl Holders for History {
    fn holders(&mut self, slot: &UniqueSlot) -> Result<Vec<Revision>, Infallible> {
        Ok(self.holders.get(slot).cloned().unwrap_or_default())
    }
}
