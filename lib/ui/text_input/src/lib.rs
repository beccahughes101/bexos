//! Shared text input component.

use bexos_dioxus_dom::{LISTENER_FOCUS, LISTENER_KEYBOARD, LISTENER_TEXT, Node};
use bexos_ui_core::{STATE_DISABLED, STATE_FOCUS};

pub const CLASS: &str = "bex-text-input";
pub const CLASS_PLACEHOLDER: &str = "bex-text-placeholder";
pub const CLASS_ERROR: &str = "bex-text-error";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextInput {
    pub id: u64,
    pub value: String,
    pub placeholder: String,
    pub masked: bool,
    pub focused: bool,
    pub disabled: bool,
    pub error: bool,
}

impl TextInput {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            value: String::new(),
            placeholder: String::new(),
            masked: false,
            focused: false,
            disabled: false,
            error: false,
        }
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    pub fn placeholder(mut self, value: impl Into<String>) -> Self {
        self.placeholder = value.into();
        self
    }

    pub fn masked(mut self, value: bool) -> Self {
        self.masked = value;
        self
    }

    pub fn focused(mut self, value: bool) -> Self {
        self.focused = value;
        self
    }

    pub fn error(mut self, value: bool) -> Self {
        self.error = value;
        self
    }

    pub fn disabled(mut self, value: bool) -> Self {
        self.disabled = value;
        self
    }

    pub fn node(self) -> Node {
        let value_empty = self.value.is_empty();
        let mut state = 0;
        if self.focused {
            state |= STATE_FOCUS;
        }
        if self.disabled {
            state |= STATE_DISABLED;
        }
        let label = if value_empty {
            self.placeholder
        } else if self.masked {
            "*".repeat(self.value.chars().count().min(64))
        } else {
            self.value
        };
        let mut node = Node::element(self.id, "input")
            .class(CLASS)
            .attribute("role", "textbox")
            .attribute(
                "aria-disabled",
                if self.disabled { "true" } else { "false" },
            )
            .listeners(if self.disabled {
                0
            } else {
                LISTENER_KEYBOARD | LISTENER_TEXT | LISTENER_FOCUS
            })
            .state(state)
            .child(Node::text(self.id + 1, label));
        if value_empty {
            node = node.class(CLASS_PLACEHOLDER);
        }
        if self.error {
            node = node.class(CLASS_ERROR);
        }
        node
    }
}

pub fn css() -> &'static str {
    r#"
.bex-text-input { background: var(--bex-input-bg, #233044); color: var(--bex-ink, #eef5ff); padding:10px; border-radius:4px; font-size:18px; }
.bex-text-input:focus { background: var(--bex-input-focus, #2f405a); }
.bex-text-placeholder { color: var(--bex-muted-ink, #85a0b8); }
.bex-text-error { background:#3d2730; }
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masked_input_never_exposes_value() {
        let node = TextInput::new(20)
            .value("secret")
            .masked(true)
            .focused(true)
            .node();
        assert!(node.state & STATE_FOCUS != 0);
        assert!(format!("{node:?}").contains("******"));
        assert!(!format!("{node:?}").contains("secret"));
    }
}
