//! The catalog: what each product declares about its references (its kinds,
//! their web sub-views, its web address and its provider-URL patterns), and
//! everything computed from it: input normalised to the global form, the
//! classification of what a person typed or pasted, a reference's web link,
//! and a web address read back as a reference.
//!
//! A product's web address for an object is `/<kind>/<id>[@<revision>]`,
//! percent-encoded once, under the product's base URL, with an optional
//! `/<sub-view>` after it and the selector as the fragment. Local input forms
//! do not change this route or the stored global reference.

use crate::escape;
use crate::locator::{Locator, LocatorError, LocatorKind};
use crate::pattern::UrlPattern;
use crate::reference::{
    Object, Product, Reference, ReferenceError, Target, Written, is_kind_name, parse_object,
};
use crate::selector::Selector;
use std::fmt;

/// One kind of object a product owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kind {
    name: String,
    views: Vec<String>,
    local_form: LocalForm,
}

/// The local spelling of a kind. Global object syntax is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalForm {
    /// The escaped `<id>[@revision][#selector]` object grammar.
    Object,
    /// A literal identity and a required `#<ordinal>`.
    Ordinal,
    /// A literal identity, optionally followed by `#<ordinal>`.
    OptionalOrdinal,
    /// A literal root, optional `@revision` and optional `#<ordinal>`.
    RevisionOrdinal,
}

impl Kind {
    /// A kind and the names of its web sub-views (`trace`, `history`, …).
    pub fn new(name: &str, views: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            views: views.iter().map(|view| view.to_string()).collect(),
            local_form: LocalForm::Object,
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn views(&self) -> &[String] {
        &self.views
    }
    pub fn with_local_form(mut self, form: LocalForm) -> Self {
        self.local_form = form;
        self
    }
}

/// What one product declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductCatalog {
    product: Product,
    kinds: Vec<Kind>,
    web: Option<String>,
    patterns: Vec<UrlPattern>,
}

impl ProductCatalog {
    pub fn new(product: Product, kinds: Vec<Kind>) -> Self {
        Self {
            product,
            kinds,
            web: None,
            patterns: Vec::new(),
        }
    }

    /// The product's web base URL, from deployment configuration. Without
    /// one, the product's references render as text.
    pub fn with_web(mut self, base: &str) -> Self {
        self.web = Some(base.trim_end_matches('/').to_owned());
        self
    }

    /// Provider-URL patterns whose output uses this product's escaped object
    /// grammar (`session:chatgpt:{id}` or `result:{id}#ordinal={row}`),
    /// independently of a kind's human local form.
    pub fn with_patterns(mut self, patterns: Vec<UrlPattern>) -> Self {
        self.patterns = patterns;
        self
    }

    pub fn product(&self) -> Product {
        self.product
    }
    pub fn kinds(&self) -> &[Kind] {
        &self.kinds
    }
    pub fn web(&self) -> Option<&str> {
        self.web.as_deref()
    }

    pub fn kind(&self, name: &str) -> Option<&Kind> {
        self.kinds.iter().find(|kind| kind.name == name)
    }
}

/// Every product's declarations, read from one product (`home`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    home: Product,
    products: Vec<ProductCatalog>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    #[error("{product} declares `{kind}`, which is not a kind name")]
    BadKind { product: Product, kind: String },
    #[error(
        "{product} declares `{kind}`, which is a reserved word (a locator kind or product name)"
    )]
    Reserved { product: Product, kind: String },
    #[error("{product} declares `{kind}` twice")]
    Repeated { product: Product, kind: String },
    #[error("{product}'s `{kind}` declares the view `{view}`, which is not a lowercase word")]
    BadView {
        product: Product,
        kind: String,
        view: String,
    },
    #[error("{0} is declared twice")]
    RepeatedProduct(Product),
    #[error("{product}'s web address `{base}` is not an absolute http(s) URL")]
    BadWeb { product: Product, base: String },
}

/// What a person typed or pasted where a reference may stand, classified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// A reference in any written form, or a URL that names one.
    Reference(Reference),
    /// A word the caller's context resolves.
    Context(ContextWord),
    /// Rows of the last list printed for this caller.
    Rows(RowPick),
    /// Anything else: a native identity or a prefix of one, which the
    /// product resolves against its data.
    Bare(String),
}

/// The words both products resolve from the caller's context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextWord {
    /// `self`: the calling session.
    CallingSession,
    /// `here`: the caller's working-directory scope (repository, worktree,
    /// project).
    Here,
    /// `last`: the product's most recent object of the kind asked for.
    Last,
    /// `latest`: the same as `last` in every product today; kept as a word
    /// because both products accept it.
    Latest,
}

impl ContextWord {
    pub fn word(self) -> &'static str {
        match self {
            Self::CallingSession => "self",
            Self::Here => "here",
            Self::Last => "last",
            Self::Latest => "latest",
        }
    }
    pub fn from_word(word: &str) -> Option<Self> {
        [Self::CallingSession, Self::Here, Self::Last, Self::Latest]
            .into_iter()
            .find(|candidate| candidate.word() == word)
    }
}

/// `@…`: rows of the last list printed in this terminal or session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowPick {
    /// `@last`: the last answer printed here.
    LastAnswer,
    /// `@*`: every row of the last list as printed.
    All,
    /// `@3`, `@3.2`, `@2-5`, `@1,4`: the listed rows, in the order written.
    Rows(Vec<RowSpan>),
}

/// One item of a row pick. Rows and hits count from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowSpan {
    /// `@3`
    Row(u64),
    /// `@3.2`: hit 2 of row 3.
    Hit { row: u64, hit: u64 },
    /// `@2-5`: rows 2 to 5, both included.
    Range { first: u64, last: u64 },
}

impl fmt::Display for RowPick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LastAnswer => f.write_str("@last"),
            Self::All => f.write_str("@*"),
            Self::Rows(spans) => {
                let spans: Vec<String> = spans
                    .iter()
                    .map(|span| match span {
                        RowSpan::Row(row) => row.to_string(),
                        RowSpan::Hit { row, hit } => format!("{row}.{hit}"),
                        RowSpan::Range { first, last } => format!("{first}-{last}"),
                    })
                    .collect();
                write!(f, "@{}", spans.join(","))
            }
        }
    }
}

impl RowPick {
    /// Parses `@last`, `@*` or a comma-separated list of `N`, `N.M` and
    /// `N-M`, all counting from 1.
    pub fn parse(text: &str) -> Option<Self> {
        let body = text.strip_prefix('@')?;
        match body {
            "last" => return Some(Self::LastAnswer),
            "*" => return Some(Self::All),
            _ => {}
        }
        let number = |text: &str| -> Option<u64> {
            (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| text.parse().ok())
                .flatten()
                .filter(|&n| n > 0)
        };
        let spans = body
            .split(',')
            .map(|item| {
                if let Some((row, hit)) = item.split_once('.') {
                    Some(RowSpan::Hit {
                        row: number(row)?,
                        hit: number(hit)?,
                    })
                } else if let Some((first, last)) = item.split_once('-') {
                    let (first, last) = (number(first)?, number(last)?);
                    (first <= last).then_some(RowSpan::Range { first, last })
                } else {
                    Some(RowSpan::Row(number(item)?))
                }
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self::Rows(spans))
    }
}

/// How a reference is shown: a link, or text with the reason it has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Url(String),
    Text(Unlinked),
}

/// Why a reference renders as text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unlinked {
    /// The owning product has no web address configured here.
    #[error("{} not configured", .0.title())]
    NotConfigured(Product),
    /// The thing has no web view (a file, an attachment, an ISBN).
    #[error("no web view")]
    NoWebView,
    /// The sub-view is not one the kind declares.
    #[error("{kind} has no `{view}` view")]
    UnknownView { kind: String, view: String },
}

/// A product web address read back as a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebAddress {
    pub reference: Reference,
    pub view: Option<String>,
    /// The query string, which carries read views and toggles; never part of
    /// the reference.
    pub parameters: Option<String>,
}

impl Catalog {
    /// A catalog read from `home`. Each product appears at most once; a
    /// product that is absent is not checked (its kinds are accepted as
    /// written) and has no web address.
    pub fn new(home: Product, products: Vec<ProductCatalog>) -> Result<Self, CatalogError> {
        for (index, catalog) in products.iter().enumerate() {
            let product = catalog.product;
            if products[..index]
                .iter()
                .any(|other| other.product == product)
            {
                return Err(CatalogError::RepeatedProduct(product));
            }
            for (index_of_kind, kind) in catalog.kinds.iter().enumerate() {
                let name = kind.name.clone();
                if !is_kind_name(&name) {
                    return Err(CatalogError::BadKind {
                        product,
                        kind: name,
                    });
                }
                if LocatorKind::from_name(&name).is_some() || Product::from_name(&name).is_some() {
                    return Err(CatalogError::Reserved {
                        product,
                        kind: name,
                    });
                }
                if catalog.kinds[..index_of_kind]
                    .iter()
                    .any(|other| other.name == name)
                {
                    return Err(CatalogError::Repeated {
                        product,
                        kind: name,
                    });
                }
                if let Some(view) = kind.views.iter().find(|view| !is_kind_name(view)) {
                    return Err(CatalogError::BadView {
                        product,
                        kind: name,
                        view: view.clone(),
                    });
                }
            }
            if let Some(base) = &catalog.web {
                let absolute = url::Url::parse(base).is_ok_and(|url| {
                    matches!(url.scheme(), "http" | "https")
                        && url.query().is_none()
                        && url.fragment().is_none()
                });
                if !absolute {
                    return Err(CatalogError::BadWeb {
                        product,
                        base: base.clone(),
                    });
                }
            }
        }
        Ok(Self { home, products })
    }

    pub fn home(&self) -> Product {
        self.home
    }

    pub fn product(&self, product: Product) -> Option<&ProductCatalog> {
        self.products
            .iter()
            .find(|catalog| catalog.product == product)
    }

    /// Parses a reference written in any form: global, local to `home`, or a
    /// locator. The result is the global reference. A local form whose kind
    /// `home` does not declare is refused, with the global form when the
    /// other product declares it.
    pub fn parse(&self, text: &str) -> Result<Reference, ReferenceError> {
        match Written::split(text)? {
            Written::Global { product, rest } => {
                let (kind, tail) =
                    rest.split_once('/')
                        .ok_or_else(|| ReferenceError::NoKindAfterProduct {
                            text: text.to_owned(),
                            product,
                        })?;
                self.check_kind(product, kind)?;
                parse_object(text, product, kind, tail)
            }
            Written::Locator { text } => Ok(Reference::locator(Locator::parse(text)?)),
            Written::Local { kind, rest } => {
                if let Some(home) = self.product(self.home)
                    && home.kind(kind).is_none()
                {
                    let owner = self.products.iter().find(|catalog| {
                        catalog.product != self.home && catalog.kind(kind).is_some()
                    });
                    return Err(match owner {
                        Some(owner) => ReferenceError::OtherProductKind {
                            kind: kind.to_owned(),
                            home: self.home,
                            owner: owner.product,
                            suggestion: format!("{}:{kind}/{rest}", owner.product),
                        },
                        None => ReferenceError::UnknownKind {
                            product: self.home,
                            kind: kind.to_owned(),
                        },
                    });
                }
                match self
                    .product(self.home)
                    .and_then(|product| product.kind(kind))
                {
                    Some(declaration) => parse_local(text, self.home, declaration, rest),
                    None => parse_object(text, self.home, kind, rest),
                }
            }
        }
    }

    fn check_kind(&self, product: Product, kind: &str) -> Result<(), ReferenceError> {
        match self.product(product) {
            Some(catalog) if catalog.kind(kind).is_none() => Err(ReferenceError::UnknownKind {
                product,
                kind: kind.to_owned(),
            }),
            _ => Ok(()),
        }
    }

    /// Print local forms using the same declarations that parse them.
    pub fn written(&self, reference: &Reference) -> String {
        match reference.target() {
            Target::Locator(locator)
                if self.home == Product::Polylogue && locator.kind() == LocatorKind::File =>
            {
                format!("<{}>", reference.global())
            }
            Target::Object(object) if object.product() == self.home => {
                let form = self
                    .product(self.home)
                    .and_then(|product| product.kind(object.kind()))
                    .map_or(LocalForm::Object, |kind| kind.local_form);
                if form == LocalForm::Object {
                    let mut text = format!("{}:{}", object.kind(), object.written_tail());
                    if let Some(selector) = reference.selector() {
                        text.push('#');
                        text.push_str(&selector.to_string());
                    }
                    return text;
                }
                let selector = reference.selector().cloned().unwrap_or_default();
                let only_ordinal = Selector {
                    ordinal: selector.ordinal,
                    ..Default::default()
                };
                let needs_global = selector != only_ordinal
                    || object
                        .id()
                        .chars()
                        .any(|c| c.is_whitespace() || c.is_control())
                    || object.revision().is_some_and(|revision| {
                        revision
                            .chars()
                            .any(|c| c.is_whitespace() || c.is_control())
                    })
                    || (selector.ordinal.is_none() && object.id().contains('#'))
                    || (form != LocalForm::RevisionOrdinal && object.revision().is_some())
                    || (form == LocalForm::Ordinal && selector.ordinal.is_none())
                    || (form == LocalForm::RevisionOrdinal
                        && (object.revision().is_none() && object.id().contains('@')
                            || object.revision().is_some_and(|revision| {
                                revision.contains('@')
                                    || selector.ordinal.is_none() && revision.contains('#')
                            })));
                if needs_global {
                    return format!("<{}>", reference.global());
                }
                let mut text = format!("{}:{}", object.kind(), object.id());
                if let Some(revision) = object.revision() {
                    text.push('@');
                    text.push_str(revision);
                }
                if let Some(ordinal) = selector.ordinal {
                    text.push('#');
                    text.push_str(&ordinal.to_string());
                }
                text
            }
            _ => reference.global(),
        }
    }

    /// Classifies input where a reference may stand: a context word, a row
    /// pick, a URL (a product's own web address, a product's provider URL, a
    /// catalog URL, or any other URL as a `url:` locator), a reference in any
    /// written form, or bare text for the product to resolve.
    pub fn read_input(&self, text: &str) -> Result<Input, ReferenceError> {
        let text = text.trim();
        if text.starts_with('<') {
            return self.parse(text).map(Input::Reference);
        }
        if let Some(word) = ContextWord::from_word(text) {
            return Ok(Input::Context(word));
        }
        if text.starts_with('@') {
            return RowPick::parse(text)
                .map(Input::Rows)
                .ok_or_else(|| ReferenceError::BadRowPick(text.to_owned()));
        }
        let head = text.split_once(':').map(|(head, _)| head);
        let known_head = head.is_some_and(|head| {
            Product::from_name(head).is_some()
                || LocatorKind::from_name(head).is_some()
                || self
                    .products
                    .iter()
                    .any(|catalog| catalog.kind(head).is_some())
        });
        if known_head {
            return self.parse(text).map(Input::Reference);
        }
        if text.contains("://") {
            return self.read_url(text).map(Input::Reference);
        }
        Ok(Input::Bare(text.to_owned()))
    }

    /// A pasted address as a reference: a product's web address first, then
    /// the products' provider-URL patterns (home first), then the shared
    /// canonicalizer.
    pub fn read_url(&self, text: &str) -> Result<Reference, ReferenceError> {
        if let Some(address) = self.read_web_address(text) {
            return Ok(address.reference);
        }
        let located = Locator::from_url(text)?;
        if located.locator.kind() == LocatorKind::Url {
            let url = url::Url::parse(located.locator.value()).map_err(|error| {
                LocatorError::NotAbsoluteUrl {
                    text: text.to_owned(),
                    reason: error.to_string(),
                }
            })?;
            let home_first = self
                .products
                .iter()
                .filter(|catalog| catalog.product == self.home)
                .chain(
                    self.products
                        .iter()
                        .filter(|catalog| catalog.product != self.home),
                );
            // Literal native identifiers keep their URL spelling, before the
            // locator canonicalizer decodes unreserved escapes or changes case.
            let original = url::Url::parse(text).map_err(|error| LocatorError::NotAbsoluteUrl {
                text: text.to_owned(),
                reason: error.to_string(),
            })?;
            for catalog in home_first {
                for pattern in &catalog.patterns {
                    let matched_url = if pattern.has_literal_captures() {
                        &original
                    } else {
                        &url
                    };
                    if let Some(written) = pattern.apply(matched_url)
                        && let Some((kind, rest)) = written.split_once(':')
                        && catalog.kind(kind).is_some()
                        && let Ok(reference) = parse_object(&written, catalog.product, kind, rest)
                    {
                        let selector = match (reference.selector(), located.selector.as_ref()) {
                            (Some(pattern), Some(fragment)) => {
                                Some(pattern.clone().merge(fragment.clone())?)
                            }
                            (Some(pattern), None) => Some(pattern.clone()),
                            (None, fragment) => fragment.cloned(),
                        };
                        return Ok(reference.with_selector(selector));
                    }
                }
            }
        }
        Ok(Reference::locator(located))
    }

    /// How `reference` is shown here: its owner's web address, or text with
    /// the reason.
    pub fn link(&self, reference: &Reference) -> Link {
        match self.web_address(reference, None) {
            Ok(url) => Link::Url(url),
            Err(reason) => Link::Text(reason),
        }
    }

    /// The web address of `reference`, optionally at one of its kind's
    /// sub-views.
    pub fn web_address(
        &self,
        reference: &Reference,
        view: Option<&str>,
    ) -> Result<String, Unlinked> {
        let object = match reference.target() {
            Target::Locator(locator) => {
                return locator
                    .web_url(reference.selector())
                    .ok_or(Unlinked::NoWebView);
            }
            Target::Object(object) => object,
        };
        let catalog = self.product(object.product());
        let base = catalog
            .and_then(ProductCatalog::web)
            .ok_or(Unlinked::NotConfigured(object.product()))?;
        if let Some(view) = view {
            let declared = catalog
                .and_then(|catalog| catalog.kind(object.kind()))
                .is_some_and(|kind| kind.views.iter().any(|declared| declared == view));
            if !declared {
                return Err(Unlinked::UnknownView {
                    kind: object.kind().to_owned(),
                    view: view.to_owned(),
                });
            }
        }
        let mut url = format!(
            "{base}/{}/{}",
            object.kind(),
            escape::encode(object.id(), escape::plain_in_path_segment)
        );
        if let Some(revision) = object.revision() {
            url.push('@');
            url.push_str(&escape::encode(revision, escape::plain_in_path_segment));
        }
        if let Some(view) = view {
            url.push('/');
            url.push_str(view);
        }
        if let Some(selector) = reference.selector() {
            url.push('#');
            url.push_str(&selector.to_string());
        }
        Ok(url)
    }

    /// Reads a product web address back as a reference: a full URL under a
    /// configured base, or a path (`/session/…`) in `home`. `None` when the
    /// address is not an object route of a configured product.
    pub fn read_web_address(&self, text: &str) -> Option<WebAddress> {
        let (product, path) = if text.starts_with('/') {
            (self.home, text)
        } else {
            self.products.iter().find_map(|catalog| {
                let base = catalog.web.as_deref()?;
                let path = text.strip_prefix(base)?;
                path.starts_with('/').then_some((catalog.product, path))
            })?
        };
        let catalog = self.product(product);
        let (path, selector) = match path.split_once('#') {
            Some((path, fragment)) => (path, Some(Selector::parse(fragment).ok()?)),
            None => (path, None),
        };
        let (path, parameters) = match path.split_once('?') {
            Some((path, query)) => (
                path,
                Some(query.to_owned()).filter(|query| !query.is_empty()),
            ),
            None => (path, None),
        };
        let mut segments = path.strip_prefix('/')?.split('/');
        let kind = segments.next()?;
        let named = segments.next()?;
        let view = segments.next();
        if segments.next().is_some() {
            return None;
        }
        if let Some(catalog) = catalog {
            let declared = catalog.kind(kind)?;
            if let Some(view) = view
                && !declared.views.iter().any(|name| name == view)
            {
                return None;
            }
        }
        let (id, revision) = match named.split_once('@') {
            Some((id, revision)) => (id, Some(revision)),
            None => (named, None),
        };
        let id = escape::decode(id).ok()?;
        let revision = revision.map(escape::decode).transpose().ok()?;
        let object = Object::new(product, kind, &id, revision.as_deref()).ok()?;
        Some(WebAddress {
            reference: Reference::new(Target::Object(object), selector),
            view: view.map(str::to_owned),
            parameters,
        })
    }
}

fn parse_local(
    text: &str,
    product: Product,
    kind: &Kind,
    tail: &str,
) -> Result<Reference, ReferenceError> {
    if kind.local_form == LocalForm::Object {
        return parse_object(text, product, &kind.name, tail);
    }
    let (identity, ordinal) = match tail.rsplit_once('#') {
        Some((identity, ordinal)) => (identity, Some(crate::selector::parse_ordinal(ordinal)?)),
        None if kind.local_form == LocalForm::Ordinal => {
            return Err(ReferenceError::MissingOrdinal(text.to_owned()));
        }
        None => (tail, None),
    };
    let (id, revision) = if kind.local_form == LocalForm::RevisionOrdinal {
        match identity.rsplit_once('@') {
            Some((id, revision)) => (id, Some(revision)),
            None => (identity, None),
        }
    } else {
        (identity, None)
    };
    Ok(Reference::new(
        Target::Object(Object::new(product, &kind.name, id, revision)?),
        Some(Selector {
            ordinal,
            ..Default::default()
        }),
    ))
}
