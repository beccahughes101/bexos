//! Locale context for retained documents. Poll once before producing a dirty
//! frame; all text in that frame resolves against the same committed snapshot.
pub use bexos_locale_catalog::{Arguments, Error, Formatter, Value};
use bexos_locale_catalog::{Catalog, Resolver};
pub use bexos_locale_settings::Settings;
pub trait Backend: Formatter {
    fn snapshot(&self) -> Result<(u64, Settings), Error>;
    fn direction(&self, language: &str) -> Result<&'static str, Error>;
}
pub struct LocaleContext<B> {
    catalog: Catalog<'static>,
    backend: B,
    generation: u64,
    settings: Settings,
    direction: &'static str,
}
pub type I18nProvider<B> = LocaleContext<B>;
impl<B: Backend> LocaleContext<B> {
    pub fn load(catalog: &'static [u8], backend: B) -> Result<Self, Error> {
        let catalog = Catalog::parse(catalog)?;
        let (generation, settings) = backend.snapshot()?;
        let settings = settings
            .with_wire(|w| Settings::from_wire(&w))
            .map_err(|_| Error::Malformed)?;
        let direction = backend.direction(&settings.languages[0])?;
        Ok(Self {
            catalog,
            backend,
            generation,
            settings,
            direction,
        })
    }
    pub fn poll(&mut self) -> Result<bool, Error> {
        let (generation, settings) = self.backend.snapshot()?;
        if generation < self.generation {
            return Ok(false);
        }
        if generation == self.generation && settings == self.settings {
            return Ok(false);
        }
        let settings = settings
            .with_wire(|w| Settings::from_wire(&w))
            .map_err(|_| Error::Malformed)?;
        let direction = self.backend.direction(&settings.languages[0])?;
        self.generation = generation;
        self.settings = settings;
        self.direction = direction;
        Ok(true)
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    pub fn direction(&self) -> &'static str {
        self.direction
    }
    pub fn text(&self, key: &str, args: &Arguments) -> String {
        struct Borrowed<'a, B>(&'a B);
        impl<B: Formatter> Formatter for Borrowed<'_, B> {
            fn number(&self, v: &str, o: &Arguments) -> Result<String, Error> {
                self.0.number(v, o)
            }
            fn datetime(&self, v: &Value, o: &Arguments) -> Result<String, Error> {
                self.0.datetime(v, o)
            }
            fn plural(&self, l: &str, v: &str) -> Result<&'static str, Error> {
                self.0.plural(l, v)
            }
        }
        Resolver {
            catalog: self.catalog,
            languages: self.settings.languages.clone(),
            formatter: Borrowed(&self.backend),
        }
        .text(key, args)
    }
}
pub fn use_locale<B: Backend>(context: &LocaleContext<B>) -> &Settings {
    context.settings()
}
#[macro_export]
macro_rules! t {
    ($context:expr, $key:expr $(, $name:ident = $value:expr)* $(,)?)=>{
        $context.text($key,&$crate::Arguments::from([$( (stringify!($name).to_string(),$crate::Value::from($value)) ),*]))
    };
}
#[cfg(test)]
mod tests;
