//! Atomic requests: one request carries several writes, numbered
//! `<request>.<n>`, which are placed together or not at all.
//!
//! A [`Plan`] places each write against the product's links plus the
//! request's own earlier writes: explicit bases are checked for staleness,
//! uniqueness slots turn a write into a revision of the record that holds the
//! slot, and withdrawals must name records that exist. The product then
//! commits [`Planned::records`] in one transaction and recomputes its stored
//! currency for [`Planned::affected_roots`]. A plan that refused any write
//! never yields records.
use crate::currency::{Currency, CurrencyError, Links, affected_roots};
use crate::record::{Header, Revision, Target};
use crate::uniqueness::{Holders, Unique, UniqueSlot, WhenHeld};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The ID of the n-th of `count` writes in a request (n from 1). A request of
/// one write uses the request ID itself.
pub fn write_id(request: &str, n: usize, count: usize) -> String {
    if count == 1 {
        request.to_owned()
    } else {
        format!("{request}.{n}")
    }
}

/// What a write naming a base that is no longer current does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnStale {
    /// Refuse the request, naming the current heads. Commands from the CLI,
    /// MCP and the web refuse.
    Refuse,
    /// Write it anyway as another head, a visible conflict. Markup and
    /// imports fork, so the order of arrival cannot change the result.
    Fork,
}

/// How a request arrived, computed by the product: the author and channel
/// come from the authenticated caller, never from the request's content, and
/// every record of the request carries them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrival {
    pub author: String,
    pub via: String,
    /// The writer's clock, Unix milliseconds.
    pub at: i64,
    pub on_stale: OnStale,
}

/// One write of a request, after the product has validated its shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Write<B> {
    pub kind: String,
    pub key: Option<String>,
    pub value: Option<serde_json::Value>,
    /// The product's own part of the record.
    pub body: B,
    /// Explicit bases: the write is a revision of these revisions, all of one
    /// record.
    pub revises: Vec<Revision>,
    pub withdraws: Vec<Target>,
    /// The slots of the written content, from the kind's
    /// [`Uniqueness`](crate::Uniqueness); `None` for a kind with many.
    pub unique: Option<Unique>,
}

/// One planned record: its header, the product's body and the slots it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Record<B> {
    pub header: Header,
    pub body: B,
    pub slots: Vec<UniqueSlot>,
}

/// Where a write went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placed {
    /// A record was written; `revised` is empty for a new record.
    Written {
        revision: Revision,
        revised: Vec<Revision>,
    },
    /// The slot already says this; nothing was written.
    Already { heads: Vec<Revision> },
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal<E> {
    #[error("{base} is not current; current: {}", list(.heads))]
    Stale {
        base: Revision,
        heads: Vec<Revision>,
    },
    #[error("base {0} does not exist")]
    UnknownBase(Revision),
    #[error("withdrawal target {0} does not exist")]
    UnknownTarget(Target),
    #[error("one revision revises one record; bases name several: {}", list(.0))]
    BasesSpanRoots(Vec<Revision>),
    #[error("the slot is held by several records: {}", list(.holders))]
    Ambiguous { holders: Vec<Revision> },
    #[error("the slot is held by another record: {}", list(.holders))]
    SlotTaken { holders: Vec<Revision> },
    #[error("the request declared {declared} writes and carries {written}")]
    Count { declared: usize, written: usize },
    #[error("write {0} of this request was refused")]
    Abandoned(usize),
    #[error("withdrawals of {0} form a cycle")]
    Cycle(Revision),
    #[error("reading links failed")]
    Links(E),
}

fn list(revisions: &[Revision]) -> String {
    revisions
        .iter()
        .map(Revision::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl<E> From<CurrencyError<E>> for Refusal<E> {
    fn from(error: CurrencyError<E>) -> Self {
        match error {
            CurrencyError::Source(error) => Refusal::Links(error),
            CurrencyError::Cycle(revision) => Refusal::Cycle(revision),
        }
    }
}

/// A refusal with the number (from 1) of the write that caused it.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("write {write}: {refusal}")]
pub struct Refused<E> {
    pub write: usize,
    pub refusal: Refusal<E>,
}

/// A request placed in full, ready to commit in one transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned<B> {
    pub records: Vec<Record<B>>,
    /// One entry per write, in order.
    pub placed: Vec<Placed>,
    /// The roots whose stored currency the commit must recompute.
    pub affected_roots: BTreeSet<String>,
}

/// Places the writes of one request in order. Each write sees the earlier
/// ones, so a later write may revise or withdraw an earlier write's record.
pub struct Plan<'a, S: Holders, B> {
    request: String,
    count: usize,
    arrival: Arrival,
    view: Overlay<'a, S>,
    bodies: Vec<B>,
    placed: Vec<Placed>,
    refused: Option<usize>,
}

impl<'a, S: Holders, B> Plan<'a, S, B> {
    /// A plan for request `request` of `count` writes over the product's
    /// current links.
    pub fn new(
        request: impl Into<String>,
        count: usize,
        arrival: Arrival,
        store: &'a mut S,
    ) -> Self {
        Self {
            request: request.into(),
            count,
            arrival,
            view: Overlay {
                store,
                pending: Vec::new(),
            },
            bodies: Vec::new(),
            placed: Vec::new(),
            refused: None,
        }
    }

    /// Places the next write. After any refusal the request is abandoned:
    /// later writes and [`Plan::finish`] are refused too.
    pub fn add(&mut self, write: Write<B>) -> Result<Placed, Refused<S::Error>> {
        let n = self.placed.len() + 1;
        if let Some(first) = self.refused {
            return Err(Refused {
                write: n,
                refusal: Refusal::Abandoned(first),
            });
        }
        match self.place(n, write) {
            Ok(placed) => {
                self.placed.push(placed.clone());
                Ok(placed)
            }
            Err(refusal) => {
                self.refused = Some(n);
                Err(Refused { write: n, refusal })
            }
        }
    }

    fn place(&mut self, n: usize, write: Write<B>) -> Result<Placed, Refusal<S::Error>> {
        if n > self.count {
            return Err(Refusal::Count {
                declared: self.count,
                written: n,
            });
        }
        let id = write_id(&self.request, n, self.count);
        for target in &write.withdraws {
            if !self.exists(target)? {
                return Err(Refusal::UnknownTarget(target.clone()));
            }
        }
        let slots = write
            .unique
            .as_ref()
            .map(|unique| unique.slots.clone())
            .unwrap_or_default();
        let (root, revised) = if !write.revises.is_empty() {
            let root = self.check_bases(&write.revises)?;
            if self.arrival.on_stale == OnStale::Refuse {
                let holders = self.current_holders(&slots)?;
                let others: Vec<Revision> = holders
                    .into_values()
                    .flatten()
                    .filter(|holder| holder.root != root)
                    .collect();
                if !others.is_empty() {
                    return Err(Refusal::SlotTaken { holders: others });
                }
            }
            let mut revised = write.revises.clone();
            revised.sort();
            revised.dedup();
            (root, revised)
        } else if let Some(unique) = &write.unique {
            let holders = self.current_holders(&unique.slots)?;
            match holders.len() {
                0 => (id.clone(), Vec::new()),
                1 => {
                    let (root, heads) = holders.into_iter().next().expect("one holder");
                    if unique.when_held == WhenHeld::Already {
                        return Ok(Placed::Already { heads });
                    }
                    // Every head of the record is revised, which also
                    // resolves a conflict.
                    let heads = Currency::new(&mut self.view).heads(&root)?;
                    (root, heads)
                }
                _ if self.arrival.on_stale == OnStale::Fork => (id.clone(), Vec::new()),
                _ => {
                    return Err(Refusal::Ambiguous {
                        holders: holders.into_values().flatten().collect(),
                    });
                }
            }
        } else {
            (id.clone(), Vec::new())
        };
        let header = Header {
            root,
            rev: id,
            kind: write.kind,
            key: write.key,
            value: write.value,
            author: self.arrival.author.clone(),
            via: self.arrival.via.clone(),
            at: self.arrival.at,
            revises: revised.iter().cloned().collect(),
            withdraws: write.withdraws.into_iter().collect(),
        };
        let revision = header.revision();
        self.view.pending.push((header, slots));
        self.bodies.push(write.body);
        Ok(Placed::Written { revision, revised })
    }

    fn exists(&mut self, target: &Target) -> Result<bool, Refusal<S::Error>> {
        let revisions = self.view.revisions(target.root()).map_err(Refusal::Links)?;
        Ok(match target {
            Target::Root(_) => !revisions.is_empty(),
            Target::Revision(revision) => revisions.contains(revision),
        })
    }

    /// Checks that the bases exist, share one root, and (when refusing stale
    /// writes) are current. Returns the root.
    fn check_bases(&mut self, bases: &[Revision]) -> Result<String, Refusal<S::Error>> {
        let roots: BTreeSet<&str> = bases.iter().map(|base| base.root.as_str()).collect();
        if roots.len() > 1 {
            let mut bases = bases.to_vec();
            bases.sort();
            return Err(Refusal::BasesSpanRoots(bases));
        }
        let root = bases[0].root.clone();
        for base in bases {
            if !self.exists(&Target::Revision(base.clone()))? {
                return Err(Refusal::UnknownBase(base.clone()));
            }
        }
        if self.arrival.on_stale == OnStale::Refuse {
            let mut currency = Currency::new(&mut self.view);
            for base in bases {
                if !currency.current(base)? {
                    return Err(Refusal::Stale {
                        base: base.clone(),
                        heads: currency.heads(&root)?,
                    });
                }
            }
        }
        Ok(root)
    }

    /// The current records holding any of the slots, grouped by root.
    fn current_holders(
        &mut self,
        slots: &[UniqueSlot],
    ) -> Result<BTreeMap<String, Vec<Revision>>, Refusal<S::Error>> {
        let mut holders = BTreeSet::new();
        for slot in slots {
            holders.extend(self.view.holders(slot).map_err(Refusal::Links)?);
        }
        let mut currency = Currency::new(&mut self.view);
        let mut by_root: BTreeMap<String, Vec<Revision>> = BTreeMap::new();
        for holder in holders {
            if currency.current(&holder)? {
                by_root.entry(holder.root.clone()).or_default().push(holder);
            }
        }
        Ok(by_root)
    }

    /// The placed request. Refused when any write was refused or fewer
    /// writes than declared were added.
    pub fn finish(self) -> Result<Planned<B>, Refused<S::Error>> {
        if let Some(first) = self.refused {
            return Err(Refused {
                write: first,
                refusal: Refusal::Abandoned(first),
            });
        }
        if self.placed.len() != self.count {
            return Err(Refused {
                write: self.placed.len(),
                refusal: Refusal::Count {
                    declared: self.count,
                    written: self.placed.len(),
                },
            });
        }
        let Plan {
            mut view,
            bodies,
            placed,
            ..
        } = self;
        let mut roots = BTreeSet::new();
        for index in 0..view.pending.len() {
            let header = view.pending[index].0.clone();
            roots.extend(affected_roots(&mut view, &header).map_err(|error| Refused {
                write: index + 1,
                refusal: Refusal::Links(error),
            })?);
        }
        let records = view
            .pending
            .into_iter()
            .zip(bodies)
            .map(|((header, slots), body)| Record {
                header,
                body,
                slots,
            })
            .collect();
        Ok(Planned {
            records,
            placed,
            affected_roots: roots,
        })
    }
}

/// The product's links with the request's earlier writes added.
struct Overlay<'a, S> {
    store: &'a mut S,
    pending: Vec<(Header, Vec<UniqueSlot>)>,
}

impl<S: Holders> Links for Overlay<'_, S> {
    type Error = S::Error;

    fn revisions(&mut self, root: &str) -> Result<Vec<Revision>, Self::Error> {
        let mut found = self.store.revisions(root)?;
        found.extend(
            self.pending
                .iter()
                .filter(|(header, _)| header.root == root)
                .map(|(header, _)| header.revision()),
        );
        Ok(found)
    }

    fn revisers(&mut self, revision: &Revision) -> Result<Vec<Revision>, Self::Error> {
        let mut found = self.store.revisers(revision)?;
        found.extend(
            self.pending
                .iter()
                .filter(|(header, _)| header.revises.contains(revision))
                .map(|(header, _)| header.revision()),
        );
        Ok(found)
    }

    fn withdrawers(&mut self, target: &Target) -> Result<Vec<Revision>, Self::Error> {
        let mut found = self.store.withdrawers(target)?;
        found.extend(
            self.pending
                .iter()
                .filter(|(header, _)| header.withdraws.contains(target))
                .map(|(header, _)| header.revision()),
        );
        Ok(found)
    }

    fn withdraws(&mut self, record: &Revision) -> Result<Vec<Target>, Self::Error> {
        match self
            .pending
            .iter()
            .find(|(header, _)| &header.revision() == record)
        {
            Some((header, _)) => Ok(header.withdraws.iter().cloned().collect()),
            None => self.store.withdraws(record),
        }
    }
}

impl<S: Holders> Holders for Overlay<'_, S> {
    fn holders(&mut self, slot: &UniqueSlot) -> Result<Vec<Revision>, Self::Error> {
        let mut found = self.store.holders(slot)?;
        found.extend(
            self.pending
                .iter()
                .filter(|(_, slots)| slots.contains(slot))
                .map(|(header, _)| header.revision()),
        );
        Ok(found)
    }
}

/// The content hash of a request, for idempotent retries. Object keys are
/// sorted before hashing, so the digest does not depend on how a map was
/// built or which serializer features a build enables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
pub struct RequestDigest(pub [u8; 32]);

impl RequestDigest {
    pub fn of<T: Serialize + ?Sized>(content: &T) -> Result<Self, serde_json::Error> {
        let value = serde_json::to_value(content)?;
        let mut text = String::new();
        canonical(&value, &mut text);
        Ok(Self(*blake3::hash(text.as_bytes()).as_bytes()))
    }
}

fn canonical(value: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            out.push('{');
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical(value, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// What a request ID that arrives again means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replay {
    /// The ID is new: plan and commit the request.
    New,
    /// The same ID with the same content: answer with the original outcome
    /// (`repeat: true`) and write nothing.
    Repeat,
    /// The same ID with other content: refuse as a conflict.
    Conflict,
}

/// Classifies an arriving request against the digest stored for its ID.
pub fn replay(stored: Option<&RequestDigest>, arriving: &RequestDigest) -> Replay {
    match stored {
        None => Replay::New,
        Some(stored) if stored == arriving => Replay::Repeat,
        Some(_) => Replay::Conflict,
    }
}
