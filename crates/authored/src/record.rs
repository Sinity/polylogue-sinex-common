//! The authored-record header and the identities its links name.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// One revision of an authored record: the record's root and this revision's
/// ID. The first revision of a record has `rev == root`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Revision {
    pub root: String,
    pub rev: String,
}

impl Revision {
    pub fn new(root: impl Into<String>, rev: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            rev: rev.into(),
        }
    }

    /// The first revision of a new record, whose root is its own ID.
    pub fn first(rev: impl Into<String>) -> Self {
        let rev = rev.into();
        Self {
            root: rev.clone(),
            rev,
        }
    }

    /// Parses `<root>@<rev>`, the form a reference carries after its type.
    pub fn parse(text: &str) -> Result<Self, IdentityError> {
        let (root, rev) = text
            .split_once('@')
            .ok_or_else(|| IdentityError::MissingRevision(text.to_owned()))?;
        check_id(root)?;
        check_id(rev)?;
        Ok(Self::new(root, rev))
    }

    /// The stored reference `<type>:<root>@<rev>`, for example
    /// `assertion:<root>@<rev>` or `curation:<root>@<rev>`.
    pub fn reference(&self, record_type: &str) -> String {
        format!("{record_type}:{}@{}", self.root, self.rev)
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.root, self.rev)
    }
}

/// What a withdrawal names: one revision, or a root, which withdraws every
/// revision of that record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Root(String),
    Revision(Revision),
}

impl Target {
    pub fn root(&self) -> &str {
        match self {
            Target::Root(root) => root,
            Target::Revision(revision) => &revision.root,
        }
    }

    /// Parses `<root>` or `<root>@<rev>`.
    pub fn parse(text: &str) -> Result<Self, IdentityError> {
        if text.contains('@') {
            Revision::parse(text).map(Target::Revision)
        } else {
            check_id(text)?;
            Ok(Target::Root(text.to_owned()))
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Root(root) => f.write_str(root),
            Target::Revision(revision) => revision.fmt(f),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("`{0}` names no revision; the form is <root>@<rev>")]
    MissingRevision(String),
    #[error("an ID is empty")]
    Empty,
    #[error("ID `{0}` contains `@`, which separates a root from its revision")]
    Reserved(String),
}

fn check_id(id: &str) -> Result<(), IdentityError> {
    if id.is_empty() {
        return Err(IdentityError::Empty);
    }
    if id.contains('@') {
        return Err(IdentityError::Reserved(id.to_owned()));
    }
    Ok(())
}

/// The header every authored record carries in both products. The product
/// keeps its own body beside it (Polylogue's links, machine and expiry;
/// Sinex's target, origin and request material) and its own storage.
///
/// `author` and `via` are computed by the product from how the request
/// arrived; no caller supplies them. `at` is the recorded authored time in
/// Unix milliseconds, or unknown when it was not recorded. New requests use
/// the writer's clock. Currency never reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub root: String,
    pub rev: String,
    pub kind: String,
    pub key: Option<String>,
    pub value: Option<serde_json::Value>,
    pub author: String,
    pub via: String,
    pub at: Option<i64>,
    pub revises: BTreeSet<Revision>,
    pub withdraws: BTreeSet<Target>,
}

impl Header {
    pub fn revision(&self) -> Revision {
        Revision::new(&self.root, &self.rev)
    }
}
