//! Native backend for the same retained-document context used by WASM apps.
use crate::{Client, SERVICE};
use bexos_ui_i18n::{Arguments, Backend, Error, Formatter, Settings, Value};
pub use bexos_ui_i18n::{t, use_locale};
use bexos_userspace::{Channel, Startup};
use std::cell::RefCell;
pub type LocaleContext = bexos_ui_i18n::LocaleContext<NativeBackend>;
pub struct NativeBackend {
    client: RefCell<Client>,
    provider: Channel,
}
impl NativeBackend {
    pub fn from_startup(startup: &mut Startup) -> Result<Self, Error> {
        let provider = startup
            .service_grants
            .iter()
            .find(|g| g.service == SERVICE)
            .map(|g| Channel(g.endpoint))
            .ok_or(Error::Missing)?;
        let descriptor = startup.locale.take().ok_or(Error::Missing)?;
        Ok(Self {
            client: RefCell::new(
                Client::from_descriptor(descriptor).map_err(|_| Error::Malformed)?,
            ),
            provider,
        })
    }
}
impl Backend for NativeBackend {
    fn snapshot(&self) -> Result<(u64, Settings), Error> {
        let mut c = self.client.borrow_mut();
        c.watch(self.provider).map_err(|_| Error::Missing)?;
        c.poll().map_err(|_| Error::Missing)?;
        Ok((c.generation, c.settings.clone()))
    }
    fn direction(&self, language: &str) -> Result<&'static str, Error> {
        let mut c = self.client.borrow_mut();
        let f = c.formatting().map_err(|_| Error::Missing)?;
        f.direction(language)
            .map(|rtl| if rtl { "rtl" } else { "ltr" })
    }
}
impl Formatter for NativeBackend {
    fn number(&self, v: &str, a: &Arguments) -> Result<String, Error> {
        self.client
            .borrow_mut()
            .formatting()
            .map_err(|_| Error::Missing)?
            .number(v, a)
    }
    fn datetime(&self, v: &Value, a: &Arguments) -> Result<String, Error> {
        self.client
            .borrow_mut()
            .formatting()
            .map_err(|_| Error::Missing)?
            .datetime(v, a)
    }
    fn plural(&self, l: &str, v: &str) -> Result<&'static str, Error> {
        self.client
            .borrow_mut()
            .formatting()
            .map_err(|_| Error::Missing)?
            .plural(l, v)
    }
}
