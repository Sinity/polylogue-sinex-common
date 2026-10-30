//! Value states retain uncertainty; only measured zero renders as zero.
use maud::{Markup, html};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueState<T> {
    Measured(T),
    Unknown { reason: Option<String> },
    NotRecorded,
    NotApplicable,
    Partial { value: T, unknown: u64 },
    Inferred(T),
    Pending,
    Error { reason: String },
}
pub fn value_cell<T: std::fmt::Display>(value: &ValueState<T>) -> Markup {
    let (state, label) = match value {
        ValueState::Measured(value) => ("measured", value.to_string()),
        ValueState::Unknown { reason } => (
            "unknown",
            reason.as_ref().map_or_else(
                || "— unknown".into(),
                |reason| format!("— unknown ({reason})"),
            ),
        ),
        ValueState::NotRecorded => ("not-recorded", "— not recorded".into()),
        ValueState::NotApplicable => ("not-applicable", "∅ n/a".into()),
        ValueState::Partial { value, unknown } => {
            ("partial", format!("≥ {value} ({unknown} unknown)"))
        }
        ValueState::Inferred(value) => ("inferred", format!("≈ {value} (inferred)")),
        ValueState::Pending => ("pending", "◷ pending".into()),
        ValueState::Error { reason } => ("error", format!("! error ({reason})")),
    };
    html! { span class="value-state" data-state=(state) { (label) } }
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

/// Shared status figures retain command-surface's absent-versus-measured states.
pub fn figure_cell<T: std::fmt::Display>(figure: &command_surface::Figure<T>) -> Markup {
    use command_surface::FigureState;
    let (state, shown) = match figure.state() {
        FigureState::Measured => (
            "measured",
            figure
                .value()
                .expect("measured figure has a value")
                .to_string(),
        ),
        FigureState::NoneFound => (
            "measured",
            format!(
                "{} (none found)",
                figure.value().expect("none_found figure has a value")
            ),
        ),
        FigureState::NotMeasured => (
            "unknown",
            format!(
                "— not measured ({})",
                figure.reason().expect("unmeasured figure has a reason")
            ),
        ),
        FigureState::Unavailable => (
            "error",
            format!(
                "! unavailable ({})",
                figure.reason().expect("unavailable figure has a reason")
            ),
        ),
        FigureState::Gap => (
            "partial",
            format!(
                "≥ gap ({})",
                figure.reason().expect("gap figure has a reason")
            ),
        ),
    };
    html! { span class="value-state" data-state=(state) data-age=(figure.age()) { (figure.name()) ": " (shown) } }
}
