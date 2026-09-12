//! Shared button component.

use bexos_dioxus_dom::{LISTENER_POINTER, Node};
use bexos_ui_core::{STATE_ACTIVE, STATE_DISABLED};

pub const CLASS: &str = "bex-button";
pub const CLASS_PRIMARY: &str = "bex-button-primary";
pub const CLASS_ACTIVE: &str = "bex-button-active";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Button {
    pub id: u64,
    pub label: String,
    pub primary: bool,
    pub active: bool,
    pub disabled: bool,
}

impl Button {
    pub fn new(id: u64, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            primary: false,
            active: false,
            disabled: false,
        }
    }

    pub fn primary(mut self, value: bool) -> Self {
        self.primary = value;
        self
    }

    pub fn active(mut self, value: bool) -> Self {
        self.active = value;
        self
    }

    pub fn disabled(mut self, value: bool) -> Self {
        self.disabled = value;
        self
    }

    pub fn node(self) -> Node {
        let mut state = 0;
        if self.active {
            state |= STATE_ACTIVE;
        }
        if self.disabled {
            state |= STATE_DISABLED;
        }
        let mut node = Node::element(self.id, "button")
            .class(CLASS)
            .listeners(if self.disabled { 0 } else { LISTENER_POINTER })
            .state(state)
            .child(Node::text(self.id + 1, self.label));
        if self.primary {
            node = node.class(CLASS_PRIMARY);
        }
        if self.active {
            node = node.class(CLASS_ACTIVE);
        }
        node
    }
}

pub fn css() -> &'static str {
    r#"
.bex-button { display:flex; flex-direction:column; background: var(--bex-button-bg, #2f405a); color: var(--bex-ink, #eef5ff); padding:8px; border-radius:6px; font-size:16px; }
.bex-button-primary { background: var(--bex-accent, #4f86c6); }
.bex-button-active, .bex-button:active { background: var(--bex-active, #6aa0d8); }
.bex-button:disabled { opacity:.55; }
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use bexos_dioxus_dom::NodeKind;

    #[test]
    fn button_sets_listener_state_and_classes() {
        let node = Button::new(10, "Open").primary(true).active(true).node();
        assert_eq!(node.listeners, LISTENER_POINTER);
        assert!(node.classes.contains(&CLASS.to_string()));
        assert!(node.classes.contains(&CLASS_PRIMARY.to_string()));
        assert!(node.state & STATE_ACTIVE != 0);
        assert!(matches!(node.kind, NodeKind::Element));
    }
}
