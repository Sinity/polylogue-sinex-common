//! One reference grammar for Polylogue and Sinex.
//!
//! - [`Reference`]: `<product>:<kind>/<id>[@<revision>][#<selector>]` in
//!   global form, `<kind>:<id>…` inside its own product, or a [`Locator`].
//!   `<global-reference>` explicitly qualifies global input without changing
//!   its canonical stored spelling.
//! - [`Selector`]: recorded member ordinals, media fragments, line ranges,
//!   text ranges, quotes and named places.
//! - [`Locator`]: canonical native addresses, from the versioned
//!   canonicalizer and its catalog patterns.
//! - [`Catalog`]: each product's kinds, web address and provider-URL
//!   patterns; input normalisation, links and web addresses.
//!
//! The crate's README states the design.

mod catalog;
mod escape;
pub mod locator;
mod pattern;
mod reference;
pub mod selector;
pub mod shared_cases;

pub use catalog::{
    Catalog, CatalogError, ContextWord, Input, Kind, Link, LocalForm, ProductCatalog, RowPick,
    RowSpan, Unlinked, WebAddress,
};
pub use locator::{Located, Locator, LocatorError, LocatorKind};
pub use pattern::{PatternError, UrlPattern};
pub use reference::{Object, Product, Reference, ReferenceError, Target, is_kind_name};
pub use selector::{Selector, SelectorError};
