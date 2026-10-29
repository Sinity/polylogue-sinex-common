//! Product-neutral document frame. All content enters as escaped maud markup.
pub use crate::keys::KEYS;
use crate::protocol::{Action, CopyForm, CopyTarget};
use maud::{DOCTYPE, Markup, html};
use std::borrow::Cow;

pub trait View<A> {
    fn render(&self, answer: &A) -> Markup;
}
impl<A, F: Fn(&A) -> Markup> View<A> for F {
    fn render(&self, answer: &A) -> Markup {
        self(answer)
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Product {
    Polylogue,
    Sinex,
}
impl Product {
    fn name(self) -> &'static str {
        match self {
            Self::Polylogue => "Polylogue",
            Self::Sinex => "Sinex",
        }
    }
    fn class(self) -> &'static str {
        match self {
            Self::Polylogue => "polylogue",
            Self::Sinex => "sinex",
        }
    }
}
pub struct Navigation<'a> {
    pub label: &'a str,
    pub href: &'a str,
    pub key: char,
    pub current: bool,
}
#[derive(Clone, Copy, Debug, Default)]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
    Dense,
}
#[derive(Clone, Copy, Debug, Default)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}
#[derive(Clone, Copy, Debug)]
pub struct Preferences {
    pub density: Density,
    pub theme: ThemeMode,
    pub shortcuts: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            density: Density::Comfortable,
            theme: ThemeMode::System,
            shortcuts: true,
        }
    }
}
pub enum DaemonState<'a> {
    Live,
    CatchingUp { waiting: u64 },
    Disconnected,
    Stopped { command: &'a str },
}
impl DaemonState<'_> {
    pub fn render(&self) -> Markup {
        html! { a href="/status" { @match self {
            Self::Live => "● live",
            Self::CatchingUp { waiting } => { "◷ catching up · " (waiting) " waiting" },
            Self::Disconnected => "! disconnected · retrying",
            Self::Stopped { command } => { "■ stopped · start: " code { (command) } }
        } } }
    }
}
pub struct Frame<'a> {
    pub preferences: Preferences,
    pub product: Product,
    pub title: &'a str,
    pub query: &'a str,
    pub query_route: &'a str,
    /// Existing operation route returning Completion[].
    pub completion_route: Option<&'a str>,
    pub navigation: &'a [Navigation<'a>],
    pub actions: &'a [Action],
    /// Existing operation view returning action HTML for selected references.
    pub actions_route: Option<&'a str>,
    pub command: Cow<'a, str>,
    pub result_handle: Option<&'a str>,
    pub feed: Option<&'a str>,
    /// Existing operation route returning the current document's fragments.
    pub refresh: Option<&'a str>,
    pub status: Markup,
    pub left: Option<Markup>,
    pub inspector: Option<Markup>,
}
impl<'a> Frame<'a> {
    pub fn new(product: Product, title: &'a str, query_route: &'a str, command: &'a str) -> Self {
        Self {
            preferences: Preferences::default(),
            product,
            title,
            query: "",
            query_route,
            completion_route: None,
            navigation: &[],
            actions: &[],
            actions_route: None,
            command: Cow::Borrowed(command),
            result_handle: None,
            feed: None,
            refresh: None,
            status: html! {},
            left: None,
            inspector: None,
        }
    }
    /// Preferred constructor: footer command derives from the operation request.
    pub fn for_request(
        product: Product,
        title: &'a str,
        query_route: &'a str,
        table: &command_surface::OperationTable,
        request: &command_surface::RunnableRequest,
    ) -> Result<Self, command_surface::Failure> {
        let mut frame = Self::new(product, title, query_route, "");
        frame.command = Cow::Owned(table.cli_command(request)?);
        Ok(frame)
    }
    pub fn render<A>(&self, answer: &A, view: &impl View<A>) -> Markup {
        let command = CopyTarget {
            label: "page".into(),
            forms: vec![CopyForm {
                label: "CLI command".into(),
                text: self.command.to_string(),
            }],
        };
        html! {
            (DOCTYPE)
            html lang="en" data-theme=[match self.preferences.theme { ThemeMode::System => None, ThemeMode::Light => Some("light"), ThemeMode::Dark => Some("dark") }] {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width, initial-scale=1";
                    title { (self.title) " · " (self.product.name()) }
                    link rel="stylesheet" href="/assets/tokens.css";
                    link rel="stylesheet" href="/assets/shell.css";
                    link rel="stylesheet" href="/assets/theme.css";
                    script type="module" src="/assets/shell.js" {}
                }
                body data-density=(match self.preferences.density { Density::Comfortable => "comfortable", Density::Compact => "compact", Density::Dense => "dense" }) data-product=(self.product.class()) data-feed=[self.feed] data-refresh=[self.refresh] data-actions-route=[self.actions_route] {
                    a class="skip" href="#main" { "Skip to content" }
                    header class="header" {
                        a class="wordmark" href="/" { (self.product.name()) }
                        nav aria-label="Sections" {
                            @for item in self.navigation {
                                a href=(item.href) data-go=(item.key) aria-current=[item.current.then_some("page")] { (item.label) }
                            }
                        }
                        form role="search" method="get" action=(self.query_route) {
                            label for="query" class="visually-hidden" { "Query" }
                            input id="query" data-query="" data-completion=[self.completion_route] name="q" type="search" value=(self.query) autocomplete="off" aria-autocomplete="list" list="query-completions";
                            datalist id="query-completions" {}
                            button type="submit" { "Find" }
                        }
                        button type="button" data-open-palette="" { ": Actions" }
                    }
                    details id="key-help" {
                        summary { "Keyboard help (?)" }
                        label { input type="checkbox" data-shortcuts="" checked[self.preferences.shortcuts]; " Single-character shortcuts" }
                        dl { @for binding in KEYS { dt { kbd { (binding.web_key()) } } dd { (binding.label) } } }
                    }
                    div class="page-frame" {
                        @if let Some(left) = &self.left { details class="rail left" open { summary { "Outline and filters" } (left) } }
                        main id="main" tabindex="-1" { (view.render(answer)) }
                        @if let Some(inspector) = &self.inspector { details class="rail inspector" open { summary { "Inspector" } (inspector) } }
                    }
                    footer {
                        (command.render(html! { code id="page-command" { (self.command.as_ref()) } }))
                        @if let Some(handle) = self.result_handle { p { "Result: " code { (handle) } } }
                    }
                    div class="status-strip" role="status" { (self.status) " · " button type="button" data-open-help="" { "? keys" } }
                    dialog id="palette" aria-labelledby="palette-title" {
                        h2 id="palette-title" { "Actions" }
                        label for="palette-query" { "Filter actions" }
                        input id="palette-query" type="search";
                        div id="palette-actions" data-palette-actions="" { @for action in self.actions { (action.render()) } }
                        @for item in self.navigation { a href=(item.href) data-palette-navigation="" { (item.label) " · g " (item.key) } }
                        button type="button" data-close-dialog="" { "Close (Esc)" }
                    }
                    dialog id="copy-menu" aria-label="Copy forms" { div data-copy-forms="" {} button type="button" data-close-dialog="" { "Close" } }
                    p id="shell-status" class="view-state" role="status" aria-live="polite" {}
                }
            }
        }
    }
}
