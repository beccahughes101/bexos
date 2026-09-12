//! Shared UI tokens and low-level document helpers.

use bexos_dioxus_dom::{Node, StyleOrigin, StyleRule};

pub const STATE_HOVER: u64 = 1 << 0;
pub const STATE_ACTIVE: u64 = 1 << 1;
pub const STATE_FOCUS: u64 = 1 << 2;
pub const STATE_DISABLED: u64 = 1 << 3;

pub const CLASS_ROOT: &str = "bex-root";
pub const CLASS_TITLE: &str = "bex-title";
pub const CLASS_PANEL: &str = "bex-panel";
pub const CLASS_MUTED: &str = "bex-muted";
pub const CLASS_ERROR: &str = "bex-error";

pub fn base_css() -> &'static str {
    r#"
main, section, div { display:flex; flex-direction:column; }
.bex-root { background: var(--bex-bg, #121a2a); color: var(--bex-ink, #eef5ff); font-size:16px; }
.bex-panel { background: var(--bex-panel, #233044); padding:20px; gap:12px; border-radius:8px; }
.bex-title { color: var(--bex-accent, #7fb0e0); font-size:48px; }
.bex-muted { color: var(--bex-muted-ink, #85a0b8); }
.bex-error { color:#ffa69e; }
"#
}

pub fn stylesheet(selector: impl Into<String>, declarations: impl Into<String>) -> StyleRule {
    StyleRule {
        selector: selector.into(),
        declarations: declarations.into(),
        origin: StyleOrigin::Author,
    }
}

pub fn text(id: u64, value: impl Into<String>, class: impl Into<String>) -> Node {
    Node::text(id, value).class(class)
}

pub fn element(id: u64, tag: impl Into<String>, class: impl Into<String>) -> Node {
    Node::element(id, tag).class(class)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_css_contains_tokens_and_shell_classes() {
        let css = base_css();
        assert!(css.contains("--bex-bg"));
        assert!(css.contains(".bex-panel"));
        assert!(css.contains(".bex-error"));
    }
}
