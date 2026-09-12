//! Resolved shell theme preferences and CSS assembly.

use bexos_component_config::{ConfigTable, schema::Value};

pub const PACKAGE_ID: &str = "bexos.ui.theme";
pub const MAX_CUSTOM_CSS: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorScheme {
    Light,
    Dark,
    HighContrast,
}

impl ColorScheme {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "high_contrast" => Some(Self::HighContrast),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::HighContrast => "high_contrast",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemePreferences {
    pub color_scheme: ColorScheme,
    pub text_scale_percent: u32,
    pub reduce_motion: bool,
    pub custom_css: String,
}

impl Default for ThemePreferences {
    fn default() -> Self {
        Self {
            color_scheme: ColorScheme::Dark,
            text_scale_percent: 100,
            reduce_motion: false,
            custom_css: String::new(),
        }
    }
}

impl ThemePreferences {
    pub fn from_table(bytes: &[u8]) -> Result<(u64, Self), &'static str> {
        let table = ConfigTable::parse(bytes).map_err(|_| "invalid config table")?;
        let mut out = Self::default();
        if let Ok(value) = table.get_string("color_scheme") {
            out.color_scheme = ColorScheme::parse(value).ok_or("invalid color scheme")?;
        }
        if let Ok(value) = table.get_u32("text_scale_percent") {
            out.text_scale_percent = value.clamp(75, 300);
        }
        if let Ok(value) = table.get_bool("reduce_motion") {
            out.reduce_motion = value;
        }
        if let Ok(value) = table.get_string("custom_css") {
            if value.len() > MAX_CUSTOM_CSS {
                return Err("custom css too large");
            }
            out.custom_css = value.into();
        }
        Ok((table.generation(), out))
    }

    pub fn user_css(&self) -> String {
        let mut css = String::new();
        css.push_str(match self.color_scheme {
            ColorScheme::Light => {
                ":root { --bex-bg:#f5f7fa; --bex-panel:#ffffff; --bex-ink:#162033; --bex-muted-ink:#56677c; --bex-accent:#2f6db3; --bex-button-bg:#dfe8f5; --bex-active:#bfd7f2; --bex-input-bg:#edf2f8; --bex-input-focus:#ffffff; }\n"
            }
            ColorScheme::Dark => {
                ":root { --bex-bg:#121a2a; --bex-panel:#233044; --bex-ink:#eef5ff; --bex-muted-ink:#85a0b8; --bex-accent:#4f86c6; --bex-button-bg:#2f405a; --bex-active:#6aa0d8; --bex-input-bg:#233044; --bex-input-focus:#2f405a; }\n"
            }
            ColorScheme::HighContrast => {
                ":root { --bex-bg:#000000; --bex-panel:#101010; --bex-ink:#ffffff; --bex-muted-ink:#ffffff; --bex-accent:#ffff00; --bex-button-bg:#000000; --bex-active:#0033ff; --bex-input-bg:#000000; --bex-input-focus:#111111; }\n"
            }
        });
        if !self.custom_css.is_empty() {
            css.push_str(&self.custom_css);
            css.push('\n');
        }
        let scale = self.text_scale_percent.clamp(75, 300) as f32 / 100.0;
        css.push_str(&format!(
            ":root {{ font-size:{}px !important; }}\n",
            16.0 * scale
        ));
        if self.color_scheme == ColorScheme::HighContrast {
            css.push_str("* { color:#ffffff !important; border-color:#ffffff !important; }\n");
            css.push_str(".bex-button-primary, .bex-button-active { color:#000000 !important; background:#ffff00 !important; }\n");
        }
        if self.reduce_motion {
            css.push_str(
                "* { transition-duration:0s !important; animation-duration:0s !important; }\n",
            );
        }
        css
    }
}

pub fn schema_defaults() -> Vec<(&'static str, Value)> {
    vec![
        ("color_scheme", Value::String("dark".into())),
        ("text_scale_percent", Value::Uint32(100)),
        ("reduce_motion", Value::Bool(false)),
        ("custom_css", Value::String(String::new())),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_orders_custom_before_accessibility_rules() {
        let prefs = ThemePreferences {
            color_scheme: ColorScheme::HighContrast,
            custom_css: ".bex-button { color:#123456; }".into(),
            ..Default::default()
        };
        let css = prefs.user_css();
        assert!(css.find("#123456").unwrap() < css.find("!important").unwrap());
    }
}
