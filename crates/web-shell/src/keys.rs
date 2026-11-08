//! The keyboard declaration used by web dispatch, help, and terminal pickers,
//! and the checks that keep each product's own keys consistent with it and
//! with the other product.
use serde::Serialize;
#[cfg(feature = "typescript")]
use ts_rs::TS;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    FocusQuery,
    Palette,
    Help,
    GoPrefix,
    GoNotices,
    GoCounterpart,
    Next,
    Previous,
    First,
    Last,
    Open,
    ToggleSelection,
    SelectVisible,
    CopyMenu,
    CopyDefault,
    CopyCommand,
    NextMatch,
    PreviousMatch,
    PanEarlier,
    PanLater,
    ZoomOut,
    ZoomIn,
    Close,
}
/// Where a shared binding acts. Timeline keys act only while focus is inside a
/// timeline; elsewhere the key keeps the browser's own meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum Place {
    Everywhere,
    Timeline,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
pub struct KeyBinding {
    pub key: &'static str,
    pub prefix: Option<&'static str>,
    pub ctrl: bool,
    pub action: KeyAction,
    /// Picker letters use Ctrl because bare letters enter the query.
    pub picker: Option<&'static str>,
    pub place: Place,
    pub label: &'static str,
}
macro_rules! binding {
    ($key:literal, $action:ident, $picker:expr, $label:literal) => {
        KeyBinding {
            key: $key,
            prefix: None,
            ctrl: false,
            action: KeyAction::$action,
            picker: $picker,
            place: Place::Everywhere,
            label: $label,
        }
    };
}
macro_rules! go {
    ($key:literal, $action:ident, $label:literal) => {
        KeyBinding {
            key: $key,
            prefix: Some("g"),
            ctrl: false,
            action: KeyAction::$action,
            picker: None,
            place: Place::Everywhere,
            label: $label,
        }
    };
}
macro_rules! timeline {
    ($key:literal, $action:ident, $label:literal) => {
        KeyBinding {
            key: $key,
            prefix: None,
            ctrl: false,
            action: KeyAction::$action,
            picker: None,
            place: Place::Timeline,
            label: $label,
        }
    };
}
pub const KEYS: &[KeyBinding] = &[
    binding!("/", FocusQuery, None, "Focus query"),
    binding!(":", Palette, None, "Actions palette"),
    KeyBinding {
        key: "k",
        prefix: None,
        ctrl: true,
        action: KeyAction::Palette,
        picker: None,
        place: Place::Everywhere,
        label: "Actions palette",
    },
    binding!("?", Help, None, "Key help"),
    binding!("g", GoPrefix, None, "Go to section (then a letter)"),
    go!("n", GoNotices, "Go to notices"),
    go!("o", GoCounterpart, "Go to this view in the other product"),
    binding!("j", Next, Some("Ctrl-j"), "Next item"),
    binding!("ArrowDown", Next, Some("ArrowDown"), "Next item"),
    binding!("k", Previous, Some("Ctrl-k"), "Previous item"),
    binding!("ArrowUp", Previous, Some("ArrowUp"), "Previous item"),
    go!("g", First, "First item"),
    binding!("G", Last, Some("Ctrl-Shift-g"), "Last item"),
    binding!(
        "Enter",
        Open,
        Some("Enter"),
        "Open focused item or enter its card"
    ),
    binding!("x", ToggleSelection, Some("Ctrl-x"), "Toggle selection"),
    binding!("*", SelectVisible, None, "Select visible items"),
    binding!("y", CopyMenu, Some("Ctrl-y"), "Copy menu"),
    binding!("Y", CopyDefault, Some("Ctrl-Shift-y"), "Copy default form"),
    KeyBinding {
        key: "c",
        prefix: Some("y"),
        ctrl: false,
        action: KeyAction::CopyCommand,
        picker: None,
        place: Place::Everywhere,
        label: "Copy CLI command",
    },
    binding!("n", NextMatch, Some("Ctrl-n"), "Next find match"),
    binding!(
        "N",
        PreviousMatch,
        Some("Ctrl-Shift-n"),
        "Previous find match"
    ),
    timeline!("ArrowLeft", PanEarlier, "Pan earlier"),
    timeline!("ArrowRight", PanLater, "Pan later"),
    timeline!("-", ZoomOut, "Zoom out"),
    timeline!("=", ZoomIn, "Zoom in"),
    binding!(
        "Escape",
        Close,
        Some("Escape"),
        "Close, leave a card and clear selection"
    ),
];
impl KeyBinding {
    pub fn web_key(&self) -> String {
        web_key(self.prefix, self.ctrl, self.key)
    }
}
fn web_key(prefix: Option<&str>, ctrl: bool, key: &str) -> String {
    format!(
        "{}{}{}",
        prefix
            .map(|prefix| format!("{prefix} "))
            .unwrap_or_default(),
        if ctrl { "Ctrl-" } else { "" },
        key
    )
}
pub fn picker_bindings() -> impl Iterator<Item = (&'static str, KeyAction)> {
    KEYS.iter()
        .filter_map(|binding| binding.picker.map(|key| (key, binding.action)))
}

/// The terminal picker form of a web key: a letter takes `Ctrl` (and `Shift`
/// for a capital, written with the lowercase letter); other keys keep their
/// name.
pub fn picker_form(key: &str) -> String {
    let mut characters = key.chars();
    match (characters.next(), characters.next()) {
        (Some(letter), None) if letter.is_ascii_lowercase() => format!("Ctrl-{letter}"),
        (Some(letter), None) if letter.is_ascii_uppercase() => {
            format!("Ctrl-Shift-{}", letter.to_ascii_lowercase())
        }
        _ => key.to_owned(),
    }
}

/// One key a product adds for its own act. `act` is a plain verb phrase that
/// names the same act the same way in both products (`add note`, `compare`).
/// A `g`-prefixed key is a navigation letter for one of the product's sections.
/// Keys inside a decision card are not listed here: a card shows its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProductKey {
    pub key: &'static str,
    pub prefix: Option<&'static str>,
    pub ctrl: bool,
    pub act: &'static str,
    /// The picker form, when the act is offered in a terminal picker.
    pub picker: Option<&'static str>,
}
impl ProductKey {
    fn web_key(&self) -> String {
        web_key(self.prefix, self.ctrl, self.key)
    }
    fn navigation(&self) -> bool {
        self.prefix == Some("g")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyConflict {
    /// A product key takes a key of the shared table.
    Shared { key: String, act: String },
    /// One product binds one key to two acts.
    TwoActs {
        key: String,
        first: String,
        second: String,
    },
    /// The two products bind one act to different keys.
    TwoKeys {
        act: String,
        first: String,
        second: String,
    },
    /// The picker form is not the web key's `Ctrl` form.
    Picker { key: String, picker: String },
}
impl std::fmt::Display for KeyConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shared { key, act } => {
                write!(f, "{act} uses {key}, which the shared key table owns")
            }
            Self::TwoActs { key, first, second } => {
                write!(f, "{key} means both {first} and {second}")
            }
            Self::TwoKeys { act, first, second } => {
                write!(
                    f,
                    "{act} is bound to {first} in one product and {second} in the other"
                )
            }
            Self::Picker { key, picker } => {
                write!(
                    f,
                    "the picker form of {key} must be its Ctrl form, not {picker}"
                )
            }
        }
    }
}

/// `same-key-same-act` within one product: no product key takes a shared key
/// (the reserved `g n` and `g o` included), a key means one act everywhere in
/// the product, and a picker form is the web key's `Ctrl` form.
pub fn check_product(keys: &[ProductKey]) -> Result<(), Vec<KeyConflict>> {
    let mut conflicts = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        if KEYS.iter().any(|binding| {
            binding.key == key.key && binding.prefix == key.prefix && binding.ctrl == key.ctrl
        }) {
            conflicts.push(KeyConflict::Shared {
                key: key.web_key(),
                act: key.act.into(),
            });
        }
        if let Some(picker) = key.picker
            && picker != picker_form(key.key)
        {
            conflicts.push(KeyConflict::Picker {
                key: key.web_key(),
                picker: picker.into(),
            });
        }
        for other in &keys[index + 1..] {
            if key.web_key() == other.web_key() && key.act != other.act {
                conflicts.push(KeyConflict::TwoActs {
                    key: key.web_key(),
                    first: key.act.into(),
                    second: other.act.into(),
                });
            }
        }
    }
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(conflicts)
    }
}

/// `same-key-same-act` between Polylogue and Sinex: a key both products bind
/// means the same act, and an act both products have takes the same keys.
/// Navigation letters name each product's own sections and are not compared.
pub fn check_between(first: &[ProductKey], second: &[ProductKey]) -> Result<(), Vec<KeyConflict>> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut conflicts = Vec::new();
    let acts = |keys: &[ProductKey]| {
        let mut acts: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        for key in keys.iter().filter(|key| !key.navigation()) {
            acts.entry(key.act).or_default().insert(key.web_key());
        }
        acts
    };
    let (first_acts, second_acts) = (acts(first), acts(second));
    for (act, first_keys) in &first_acts {
        if let Some(second_keys) = second_acts.get(act)
            && first_keys != second_keys
        {
            conflicts.push(KeyConflict::TwoKeys {
                act: (*act).into(),
                first: first_keys.iter().cloned().collect::<Vec<_>>().join(", "),
                second: second_keys.iter().cloned().collect::<Vec<_>>().join(", "),
            });
        }
    }
    for left in first.iter().filter(|key| !key.navigation()) {
        for right in second.iter().filter(|key| !key.navigation()) {
            if left.web_key() == right.web_key() && left.act != right.act {
                conflicts.push(KeyConflict::TwoActs {
                    key: left.web_key(),
                    first: left.act.into(),
                    second: right.act.into(),
                });
            }
        }
    }
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(conflicts)
    }
}
