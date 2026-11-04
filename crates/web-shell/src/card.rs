//! Decision cards: a key layer entered with `Enter` and left with `Esc`.
//!
//! While a card is entered, its own keys come before the shared table, and each
//! key is shown on its button. No keystroke made while moving through a list
//! reaches a card's actions: the card has to be entered first. Without script
//! every action is an ordinary form.
use crate::protocol::Action;
use maud::{Markup, html};

pub struct CardAction {
    /// One character, shown on the button. Inside the card it may be any
    /// letter, including a shared one: the card rebinds it visibly.
    pub key: char,
    /// A write operation from the product's table; its own `key` stays empty.
    pub action: Action,
}

pub struct Card {
    pub label: String,
    pub body: Markup,
    pub actions: Vec<CardAction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardError {
    /// Two actions of one card share a key.
    KeyTwice(char),
    /// A card key must be a visible character other than space.
    Key(char),
    /// A card action is a write, and it carries no list key of its own.
    NotACardAction(String),
}
impl std::fmt::Display for CardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyTwice(key) => write!(f, "two card actions use the key {key}"),
            Self::Key(key) => write!(f, "{key:?} cannot be a card key"),
            Self::NotACardAction(operation) => write!(
                f,
                "{operation} must be a write without a list key to act from a card"
            ),
        }
    }
}
impl std::error::Error for CardError {}

impl Card {
    pub fn new(
        label: impl Into<String>,
        body: Markup,
        actions: Vec<CardAction>,
    ) -> Result<Self, CardError> {
        let mut keys = std::collections::BTreeSet::new();
        for card_action in &actions {
            let key = card_action.key;
            if key.is_whitespace() || key.is_control() {
                return Err(CardError::Key(key));
            }
            if !keys.insert(key) {
                return Err(CardError::KeyTwice(key));
            }
            if !card_action.action.write || card_action.action.key.is_some() {
                return Err(CardError::NotACardAction(
                    card_action.action.operation.clone(),
                ));
            }
        }
        Ok(Self {
            label: label.into(),
            body,
            actions,
        })
    }
    pub fn render(&self) -> Markup {
        html! {
            shell-card tabindex="-1" role="group" aria-label=(self.label) {
                p class="card-label" { (self.label) }
                (self.body)
                p class="card-hint" { "Enter to use these keys · Esc to leave" }
                div class="card-actions" {
                    @for card_action in &self.actions {
                        (card_action.action.render_with_key(card_action.key))
                    }
                }
            }
        }
    }
}
