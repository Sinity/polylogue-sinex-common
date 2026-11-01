//! The reference grammar: one form for anything either product can address.
//!
//! The stored, global form of a product object is
//! `<product>:<kind>/<id>[@<revision>][#<selector>]`; inside its own product
//! it may be written `<kind>:<id>…` and is normalised to the global form at
//! input ([`crate::Catalog::parse`]). A locator (`url:`, `file:`, `youtube:`
//! …) belongs to neither product and is written the same everywhere.

use crate::escape;
use crate::locator::{Located, Locator, LocatorError, LocatorKind};
use crate::selector::{Selector, SelectorError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// The products that own objects. Their names are the global prefixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Product {
    Polylogue,
    Sinex,
}

impl Product {
    pub const ALL: [Self; 2] = [Self::Polylogue, Self::Sinex];

    pub fn name(self) -> &'static str {
        match self {
            Self::Polylogue => "polylogue",
            Self::Sinex => "sinex",
        }
    }

    /// The name as a person reads it: `Polylogue`, `Sinex`.
    pub fn title(self) -> &'static str {
        match self {
            Self::Polylogue => "Polylogue",
            Self::Sinex => "Sinex",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|product| product.name() == name)
    }
}

impl fmt::Display for Product {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One object a product owns: its kind, its identity and, when it names one
/// exact revision, that revision. What an identity and a revision look like
/// is the owning product's; the grammar only fixes where they end.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Object {
    product: Product,
    kind: String,
    id: String,
    revision: Option<String>,
}

/// What a reference names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    Object(Object),
    Locator(Locator),
}

/// A reference: a target and, optionally, a place inside it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Reference {
    target: Target,
    selector: Option<Selector>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReferenceError {
    #[error("`{0}` is not a reference: it has no `<kind>:` prefix")]
    NoKind(String),
    #[error(
        "`{0}` is not a kind name; a kind is a lowercase letter followed by letters, digits, `-` or `_`"
    )]
    BadKind(String),
    #[error("`{0}` has no identity after its kind")]
    EmptyId(String),
    #[error("`{0}` has `@` with no revision after it")]
    EmptyRevision(String),
    #[error("`{text}` is a {product} reference without its kind; write {product}:<kind>/<id>")]
    NoKindAfterProduct { text: String, product: Product },
    #[error(
        "`{text}` is written in local form; a stored or cross-product reference names its product: {suggestion}"
    )]
    LocalForm { text: String, suggestion: String },
    #[error("{product} has no `{kind}` references")]
    UnknownKind { product: Product, kind: String },
    #[error("`{kind}:` belongs to {owner}; inside {home} write the global form {suggestion}")]
    OtherProductKind {
        kind: String,
        home: Product,
        owner: Product,
        suggestion: String,
    },
    #[error("`{0}` is not a row pick; use @N, @N.M, @N-M, @N,M, @* or @last")]
    BadRowPick(String),
    #[error("`{0}`'s escapes do not decode to UTF-8 text")]
    NotUtf8(String),
    #[error(transparent)]
    Selector(#[from] SelectorError),
    #[error(transparent)]
    Locator(#[from] LocatorError),
}

/// Whether `name` is a kind name: a lowercase ASCII letter followed by
/// lowercase letters, digits, `-` or `_`.
pub fn is_kind_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

impl Object {
    /// An object reference. The kind must be a kind name that is not a
    /// locator kind or a product name; the identity must not be empty.
    pub fn new(
        product: Product,
        kind: &str,
        id: &str,
        revision: Option<&str>,
    ) -> Result<Self, ReferenceError> {
        if !is_kind_name(kind)
            || LocatorKind::from_name(kind).is_some()
            || Product::from_name(kind).is_some()
        {
            return Err(ReferenceError::BadKind(kind.to_owned()));
        }
        if id.is_empty() {
            return Err(ReferenceError::EmptyId(format!("{product}:{kind}/")));
        }
        if revision == Some("") {
            return Err(ReferenceError::EmptyRevision(format!(
                "{product}:{kind}/{id}@"
            )));
        }
        Ok(Self {
            product,
            kind: kind.to_owned(),
            id: id.to_owned(),
            revision: revision.map(str::to_owned),
        })
    }

    pub fn product(&self) -> Product {
        self.product
    }
    pub fn kind(&self) -> &str {
        &self.kind
    }
    /// The identity, decoded.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// The revision, decoded, when the reference names one.
    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    /// The same object without a revision.
    pub fn without_revision(&self) -> Self {
        Self {
            revision: None,
            ..self.clone()
        }
    }

    /// `<id>[@<revision>]` with the grammar's escapes.
    fn written_tail(&self) -> String {
        let mut text = escape::encode(&self.id, escape::plain_in_reference);
        if let Some(revision) = &self.revision {
            text.push('@');
            text.push_str(&escape::encode(revision, escape::plain_in_revision));
        }
        text
    }
}

impl Reference {
    pub fn new(target: Target, selector: Option<Selector>) -> Self {
        Self {
            target,
            selector: selector.filter(|selector| !selector.is_empty()),
        }
    }

    pub fn object(object: Object) -> Self {
        Self::new(Target::Object(object), None)
    }

    pub fn locator(located: Located) -> Self {
        Self::new(Target::Locator(located.locator), located.selector)
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    pub fn selector(&self) -> Option<&Selector> {
        self.selector.as_ref()
    }

    pub fn with_selector(self, selector: Option<Selector>) -> Self {
        Self::new(self.target, selector)
    }

    /// The owning product, for an object reference.
    pub fn product(&self) -> Option<Product> {
        match &self.target {
            Target::Object(object) => Some(object.product),
            Target::Locator(_) => None,
        }
    }

    /// Parses a reference in its global form: a product object
    /// (`sinex:ev/<id>`) or a locator (`url:…`, `youtube:…`). This is the
    /// form stored and sent between the products, so it needs no catalog. A
    /// local form is refused, since it does not say its product; input in
    /// local form goes through [`crate::Catalog::parse`].
    pub fn parse(text: &str) -> Result<Self, ReferenceError> {
        match Written::split(text)? {
            Written::Global { product, rest } => parse_global_object(text, product, rest),
            Written::Locator => Ok(Self::locator(Locator::parse(text)?)),
            Written::Local { kind, .. } => Err(ReferenceError::LocalForm {
                text: text.to_owned(),
                suggestion: format!("<product>:{kind}/…"),
            }),
        }
    }

    /// The global form: what is stored and sent.
    pub fn global(&self) -> String {
        self.to_string()
    }

    /// The form a person reads inside `home`: local for `home`'s own objects,
    /// global for everything else.
    pub fn written_in(&self, home: Product) -> String {
        match &self.target {
            Target::Object(object) if object.product == home => {
                let mut text = format!("{}:{}", object.kind, object.written_tail());
                if let Some(selector) = &self.selector {
                    text.push('#');
                    text.push_str(&selector.to_string());
                }
                text
            }
            _ => self.global(),
        }
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.target {
            Target::Object(object) => write!(
                f,
                "{}:{}/{}",
                object.product,
                object.kind,
                object.written_tail()
            )?,
            Target::Locator(locator) => write!(f, "{locator}")?,
        }
        if let Some(selector) = &self.selector {
            write!(f, "#{selector}")?;
        }
        Ok(())
    }
}

impl FromStr for Reference {
    type Err = ReferenceError;
    fn from_str(text: &str) -> Result<Self, ReferenceError> {
        Self::parse(text)
    }
}

impl Serialize for Reference {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.global())
    }
}

impl<'de> Deserialize<'de> for Reference {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// How a piece of reference text is written, before any catalog is
/// consulted.
pub(crate) enum Written<'a> {
    /// `<product>:<rest>`
    Global { product: Product, rest: &'a str },
    /// `<locator kind>:…`
    Locator,
    /// `<kind>:<rest>`
    Local { kind: &'a str, rest: &'a str },
}

impl<'a> Written<'a> {
    pub(crate) fn split(text: &'a str) -> Result<Self, ReferenceError> {
        let (head, rest) = text
            .split_once(':')
            .ok_or_else(|| ReferenceError::NoKind(text.to_owned()))?;
        if let Some(product) = Product::from_name(head) {
            return Ok(Self::Global { product, rest });
        }
        if LocatorKind::from_name(head).is_some() {
            return Ok(Self::Locator);
        }
        if !is_kind_name(head) {
            return Err(ReferenceError::BadKind(head.to_owned()));
        }
        Ok(Self::Local { kind: head, rest })
    }
}

fn parse_global_object(
    text: &str,
    product: Product,
    rest: &str,
) -> Result<Reference, ReferenceError> {
    let (kind, tail) = rest
        .split_once('/')
        .ok_or_else(|| ReferenceError::NoKindAfterProduct {
            text: text.to_owned(),
            product,
        })?;
    parse_object(text, product, kind, tail)
}

/// Parses `<id>[@<revision>][#<selector>]` for an object of `kind`.
pub(crate) fn parse_object(
    text: &str,
    product: Product,
    kind: &str,
    tail: &str,
) -> Result<Reference, ReferenceError> {
    let (before_selector, selector) = match tail.split_once('#') {
        Some((before, selector)) => (before, Some(Selector::parse(selector)?)),
        None => (tail, None),
    };
    let (id, revision) = match before_selector.split_once('@') {
        Some((id, revision)) => (id, Some(revision)),
        None => (before_selector, None),
    };
    let decode =
        |part: &str| escape::decode(part).map_err(|_| ReferenceError::NotUtf8(text.to_owned()));
    if id.is_empty() {
        return Err(ReferenceError::EmptyId(text.to_owned()));
    }
    if revision == Some("") {
        return Err(ReferenceError::EmptyRevision(text.to_owned()));
    }
    let id = decode(id)?;
    let revision = revision.map(decode).transpose()?;
    let object = Object::new(product, kind, &id, revision.as_deref())?;
    Ok(Reference::new(Target::Object(object), selector))
}
