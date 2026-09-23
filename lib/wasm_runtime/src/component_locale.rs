use crate::{
    bindings::bexos::wasm::locale,
    context::Context,
    resources::{Handle, Kind, READ, WRITE},
};
use std::sync::Arc;
use wasmtime::{Result, bail};
impl Context {
    fn locale_provider(&self, id: u32) -> Result<Arc<dyn Handle>> {
        let handle = self
            .resources
            .get(id, Kind::Channel, READ | WRITE)?
            .handle
            .clone();
        let grant = handle
            .grant()
            .ok_or_else(|| wasmtime::format_err!("locale grant missing"))?;
        if grant.service != "bexos.locale.LocaleProvider"
            || grant.protocol != "LocaleProvider"
            || grant.capability != "Public"
            || !grant.method_ordinals.contains(&1)
        {
            bail!("locale grant denied");
        }
        Ok(handle)
    }
    fn format_locale(
        &self,
        provider: u32,
        operation: u32,
        value: &str,
        options: &[(String, String)],
    ) -> Result<Result<String, String>> {
        if self.restoring {
            return Ok(Err("formatting during restore".into()));
        }
        if value.len() > 4096
            || options.len() > 256
            || options.iter().any(|(k, v)| k.len() > 64 || v.len() > 4096)
            || options
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>()
                > 32768
        {
            return Ok(Err("formatting bounds".into()));
        }
        let provider = self.locale_provider(provider)?;
        Ok(self
            .host
            .locale_format(&*provider, operation, value, options)
            .map_err(|e| e.to_string()))
    }
}
impl locale::Host for Context {
    fn snapshot(&mut self, provider: u32) -> Result<Result<Vec<u8>, String>> {
        let provider = self.locale_provider(provider)?;
        Ok(self
            .host
            .locale_snapshot(&*provider, !self.restoring)
            .map_err(|e| e.to_string()))
    }
    fn number(
        &mut self,
        provider: u32,
        value: String,
        options: Vec<locale::FormatOption>,
    ) -> Result<Result<String, String>> {
        self.format_locale(
            provider,
            0,
            &value,
            &options
                .into_iter()
                .map(|o| (o.name, o.value))
                .collect::<Vec<_>>(),
        )
    }
    fn datetime(
        &mut self,
        provider: u32,
        value: String,
        timestamp: bool,
        options: Vec<locale::FormatOption>,
    ) -> Result<Result<String, String>> {
        self.format_locale(
            provider,
            if timestamp { 2 } else { 1 },
            &value,
            &options
                .into_iter()
                .map(|o| (o.name, o.value))
                .collect::<Vec<_>>(),
        )
    }
    fn currency(
        &mut self,
        provider: u32,
        value: String,
        code: String,
    ) -> Result<Result<String, String>> {
        self.format_locale(provider, 3, &value, &[("currency".into(), code)])
    }
    fn list(
        &mut self,
        provider: u32,
        values: Vec<String>,
        disjunction: bool,
    ) -> Result<Result<String, String>> {
        self.format_locale(
            provider,
            if disjunction { 5 } else { 4 },
            "",
            &values
                .into_iter()
                .map(|v| (String::new(), v))
                .collect::<Vec<_>>(),
        )
    }
    fn plural(
        &mut self,
        provider: u32,
        language: String,
        value: String,
    ) -> Result<Result<String, String>> {
        self.format_locale(provider, 6, &value, &[("language".into(), language)])
    }
    fn direction(&mut self, provider: u32, language: String) -> Result<Result<String, String>> {
        self.format_locale(provider, 7, &language, &[])
    }
}
