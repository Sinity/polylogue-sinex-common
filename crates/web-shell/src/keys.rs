//! The keyboard declaration used by web dispatch, help, and terminal pickers.
use serde::Serialize;
use ts_rs::TS;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    FocusQuery,
    Palette,
    Help,
    GoPrefix,
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
    Close,
}
#[derive(Clone, Copy, Debug, Serialize, TS)]
pub struct KeyBinding {
    pub key: &'static str,
    pub prefix: Option<&'static str>,
    pub ctrl: bool,
    pub action: KeyAction,
    /// Picker letters use Ctrl because bare letters enter the query.
    pub picker: Option<&'static str>,
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
        label: "Actions palette",
    },
    binding!("?", Help, None, "Key help"),
    binding!("g", GoPrefix, None, "Go to section (then a letter)"),
    binding!("j", Next, Some("Ctrl-j"), "Next item"),
    binding!("ArrowDown", Next, Some("ArrowDown"), "Next item"),
    binding!("k", Previous, Some("Ctrl-k"), "Previous item"),
    binding!("ArrowUp", Previous, Some("ArrowUp"), "Previous item"),
    KeyBinding {
        key: "g",
        prefix: Some("g"),
        ctrl: false,
        action: KeyAction::First,
        picker: None,
        label: "First item",
    },
    binding!("G", Last, Some("Ctrl-Shift-g"), "Last item"),
    binding!("Enter", Open, Some("Enter"), "Open focused item"),
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
        label: "Copy CLI command",
    },
    binding!("n", NextMatch, Some("Ctrl-n"), "Next find match"),
    binding!(
        "N",
        PreviousMatch,
        Some("Ctrl-Shift-n"),
        "Previous find match"
    ),
    binding!("Escape", Close, Some("Escape"), "Close and clear selection"),
];
impl KeyBinding {
    pub fn web_key(&self) -> String {
        format!(
            "{}{}{}",
            self.prefix
                .map(|prefix| format!("{prefix} "))
                .unwrap_or_default(),
            if self.ctrl { "Ctrl-" } else { "" },
            self.key
        )
    }
}
pub fn picker_bindings() -> impl Iterator<Item = (&'static str, KeyAction)> {
    KEYS.iter()
        .filter_map(|binding| binding.picker.map(|key| (key, binding.action)))
}
