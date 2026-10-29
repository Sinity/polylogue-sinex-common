//! Shared document rendering and browser behavior. Products own views and routes;
//! this crate owns their frame, controls, assets, and live fragment protocol.
pub mod assets;
pub mod frame;
pub mod keys;
pub mod matching;
pub mod protocol;
pub mod theme;
pub mod values;

pub use frame::{Frame, Navigation, Product, View};
pub use maud::{Markup, html};
pub use protocol::{CopyForm, CopyTarget, FragmentUpdate};
pub use values::{ValueState, value_cell};
