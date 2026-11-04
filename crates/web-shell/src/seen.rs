//! The display rule for seen marks.
//!
//! A seen mark records, per consumer and target, the latest position displayed
//! to that consumer. The product renders a `<shell-seen>` element where the
//! displayed content ends, holding the write that records the mark. The shell
//! sends that write once, and only when the element is shown to a person: the
//! document is visible and not prerendering, the element is in the viewport,
//! rendered, and not inside a preview. Polling, prerendering, hidden tabs,
//! previews and agent reads of the HTML never send it. Without script the
//! element shows the write as an ordinary form.
use crate::protocol::Action;
use maud::{Markup, html};

pub struct Seen {
    action: Action,
}
impl Seen {
    /// `action` is the product's seen-mark write for this target and position.
    pub fn new(action: Action) -> Result<Self, String> {
        if !action.write {
            return Err(format!("{} is not a write", action.operation));
        }
        if action.key.is_some() {
            return Err("a seen mark is written by display, not by a key".into());
        }
        Ok(Self { action })
    }
    pub fn render(&self) -> Markup {
        html! {
            shell-seen data-action=(serde_json::to_string(&self.action).expect("action strings serialize")) {
                (self.action.render())
            }
        }
    }
}
