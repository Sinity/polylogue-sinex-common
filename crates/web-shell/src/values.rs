//! Value cells render command-surface's value states; only a value that is
//! held renders as a number.
use command_surface::ValueState;
use maud::{Markup, html};

pub fn value_cell<T: std::fmt::Display>(value: &ValueState<T>) -> Markup {
    html! { span class="value-state" data-state=(value.name().replace('_', "-")) { (value.text()) } }
}
pub fn unavailable(reason: &str, command: &str) -> Markup {
    html! { section class="unavailable" role="status" { p { "Unavailable: " (reason) } p { "Run " code { (command) } } } }
}
pub fn view_state(state: &str, reason: &str, command: &str) -> Markup {
    html! { p class="view-state" role="status" data-state=(state) { (state) ": " (reason) " · " code { (command) } } }
}
pub fn stale_banner(reason: &str) -> Markup {
    html! { p class="stale" role="status" { "◷ stale: " (reason) } }
}

/// A status figure: its name and its value cell, with the measurement's age.
pub fn figure_cell<T: std::fmt::Display>(figure: &command_surface::Figure<T>) -> Markup {
    let value = figure.value();
    html! { span class="value-state" data-state=(value.name().replace('_', "-")) data-age=(figure.age()) { (figure.name()) ": " (value.text()) } }
}
