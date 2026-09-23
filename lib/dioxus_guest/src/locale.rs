//! Application catalogs are evaluated here; only formatting crosses the WIT ABI.
use crate::bexos::wasm::{kernel, locale};
use bexos_ui_i18n::{Arguments, Backend, Error, Formatter, Settings, Value};
pub use bexos_ui_i18n::{t, use_locale};
use locale_fidl::FidlDecode;
pub type LocaleContext = bexos_ui_i18n::LocaleContext<GuestBackend>;
pub type I18nProvider = LocaleContext;
pub struct GuestBackend {
    provider: u32,
}
impl GuestBackend {
    pub fn discover() -> Result<Self, Error> {
        Ok(Self {
            provider: kernel::resource_find("bexos.locale.LocaleProvider").ok_or(Error::Missing)?,
        })
    }
    pub fn load(catalog: &'static [u8]) -> Result<LocaleContext, Error> {
        LocaleContext::load(catalog, Self::discover()?)
    }
}
fn options(args: &Arguments) -> Vec<locale::FormatOption> {
    args.iter()
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, v)| locale::FormatOption {
            name: k.clone(),
            value: match v {
                Value::Text(s) | Value::Number(s) => s.clone(),
                Value::FormattedNumber { raw, .. } => raw.clone(),
            },
        })
        .collect()
}
impl Formatter for GuestBackend {
    fn number(&self, v: &str, args: &Arguments) -> Result<String, Error> {
        locale::number(self.provider, v, &options(args)).map_err(|_| Error::Missing)
    }
    fn datetime(&self, v: &Value, args: &Arguments) -> Result<String, Error> {
        let (v, timestamp) = match v {
            Value::Text(s) => (s, false),
            Value::Number(s) | Value::FormattedNumber { raw: s, .. } => (s, true),
        };
        locale::datetime(self.provider, v, timestamp, &options(args)).map_err(|_| Error::Missing)
    }
    fn plural(&self, language: &str, value: &str) -> Result<&'static str, Error> {
        match locale::plural(self.provider, language, value)
            .map_err(|_| Error::Missing)?
            .as_str()
        {
            "zero" => Ok("zero"),
            "one" => Ok("one"),
            "two" => Ok("two"),
            "few" => Ok("few"),
            "many" => Ok("many"),
            "other" => Ok("other"),
            _ => Err(Error::Malformed),
        }
    }
}
impl Backend for GuestBackend {
    fn snapshot(&self) -> Result<(u64, Settings), Error> {
        let bytes = locale::snapshot(self.provider).map_err(|_| Error::Missing)?;
        let q = locale_fidl::LocaleSnapshot::decode(&bytes, &[]).map_err(|_| Error::Malformed)?;
        Ok((
            q.generation,
            Settings::from_wire(&q.settings).map_err(|_| Error::Malformed)?,
        ))
    }
    fn direction(&self, language: &str) -> Result<&'static str, Error> {
        match locale::direction(self.provider, language)
            .map_err(|_| Error::Missing)?
            .as_str()
        {
            "ltr" => Ok("ltr"),
            "rtl" => Ok("rtl"),
            _ => Err(Error::Malformed),
        }
    }
}

/// Refresh before rendering a frame. The runner retains the subscription across
/// application checkpoints; rebuilding this context does not create a new one.
pub fn poll(context: &mut Option<LocaleContext>, catalog: &'static [u8]) -> Result<bool, String> {
    if let Some(context) = context {
        context.poll().map_err(|e| format!("locale: {e:?}"))
    } else {
        *context = Some(GuestBackend::load(catalog).map_err(|e| format!("locale: {e:?}"))?);
        Ok(true)
    }
}
