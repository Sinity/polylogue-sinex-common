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

/// A status measurement keeps command-surface's absent-versus-measured states.
pub fn measurement_cell<T: std::fmt::Display>(
    measurement: &command_surface::Measurement<T>,
) -> Markup {
    use command_surface::MeasurementState;
    let (state, shown) = match measurement.state() {
        MeasurementState::Measured => (
            "measured",
            measurement
                .value()
                .expect("measured measurement has a value")
                .to_string(),
        ),
        MeasurementState::NoneFound => (
            "measured",
            format!(
                "{} (none found)",
                measurement
                    .value()
                    .expect("none_found measurement has a value")
            ),
        ),
        MeasurementState::NotMeasured => (
            "unknown",
            format!(
                "— not measured ({})",
                measurement
                    .reason()
                    .expect("unmeasured measurement has a reason")
            ),
        ),
        MeasurementState::Unavailable => (
            "error",
            format!(
                "! unavailable ({})",
                measurement
                    .reason()
                    .expect("unavailable measurement has a reason")
            ),
        ),
        MeasurementState::Gap => (
            "partial",
            format!(
                "≥ gap ({})",
                measurement.reason().expect("gap measurement has a reason")
            ),
        ),
    };
    html! { span class="value-state" data-state=(state) data-age=(measurement.age()) { (measurement.name()) ": " (shown) } }
}
