//! Shared document rendering and browser behavior. Products own views and routes;
//! this crate owns their frame, the shell's custom elements, assets, the live
//! fragment protocol over one connection per browser, and the timeline series.
pub mod assets;
pub mod card;
pub mod feed;
pub mod frame;
pub mod keys;
pub mod matching;
pub mod protocol;
pub mod seen;
pub mod theme;
pub mod timeline;
pub mod values;

pub use card::{Card, CardAction};
pub use command_surface::ValueState;
pub use feed::Feed;
pub use frame::{Counterpart, Frame, Navigation, Product, View};

pub use maud::{Markup, html};
pub use protocol::{CopyForm, CopyTarget, FragmentUpdate};
pub use seen::Seen;
pub use timeline::{Series, SeriesRow, Timeline};
pub use values::value_cell;
