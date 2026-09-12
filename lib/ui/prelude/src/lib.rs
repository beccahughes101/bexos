//! Public shell component prelude.

pub use bexos_dioxus_dom::{
    Document, LISTENER_FOCUS, LISTENER_KEYBOARD, LISTENER_POINTER, LISTENER_TEXT, Node, NodeKind,
    StyleRule,
};
pub use bexos_ui_button::{Button, css as button_css};
pub use bexos_ui_core::{
    CLASS_ERROR, CLASS_MUTED, CLASS_PANEL, CLASS_ROOT, CLASS_TITLE, STATE_ACTIVE, STATE_DISABLED,
    STATE_FOCUS, STATE_HOVER, base_css, element, stylesheet, text,
};
pub use bexos_ui_text_input::{TextInput, css as text_input_css};
pub use bexos_ui_theme::{ColorScheme, ThemePreferences};

pub fn component_css() -> String {
    [base_css(), button_css(), text_input_css()].join("\n")
}
