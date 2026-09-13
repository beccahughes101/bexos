use super::*;
use bexos_i18n_client::Client;
use bexos_locale_catalog::{Formatter, Value};
use bexos_userspace::startup::LocaleDescriptor;
#[derive(Default)]
pub struct LocaleState {
    pub client: Option<Client>,
}
impl LocaleState {
    pub fn install(&mut self, descriptor: Option<LocaleDescriptor>) -> Result<()> {
        if let Some(d) = descriptor {
            self.client = Some(
                Client::from_descriptor(d)
                    .map_err(|e| wasmtime::format_err!("locale descriptor {e:?}"))?,
            );
        }
        Ok(())
    }
    fn client(&mut self) -> Result<&mut Client> {
        self.client
            .as_mut()
            .ok_or_else(|| wasmtime::format_err!("locale startup absent"))
    }
    pub fn snapshot(&mut self, provider: &dyn Handle, refresh: bool) -> Result<Vec<u8>> {
        let client = self.client()?;
        if refresh {
            let grant = provider
                .grant()
                .ok_or_else(|| wasmtime::format_err!("locale grant absent"))?;
            if !grant.method_ordinals.contains(&3) {
                bail!("locale watch denied");
            }
            client
                .watch(Channel(provider.native()))
                .map_err(|e| wasmtime::format_err!("locale watch {e:?}"))?;
            client
                .poll()
                .map_err(|e| wasmtime::format_err!("locale update {e:?}"))?;
        }
        Ok(client.snapshot().to_vec())
    }
    pub fn format(
        &mut self,
        operation: u32,
        value: &str,
        options: &[(String, String)],
    ) -> Result<String> {
        let client = self.client()?;
        let f = client
            .formatting()
            .map_err(|e| wasmtime::format_err!("locale map {e:?}"))?;
        let args = options
            .iter()
            .map(|(k, v)| (k.clone(), Value::text(v)))
            .collect::<Vec<_>>();
        let option = |name| {
            options
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        let result = match operation {
            0 => f.number(value, &args),
            1 => f.datetime(&Value::text(value), &args),
            2 => f.datetime(&Value::Number(value.into()), &args),
            3 => f.currency(value, option("currency")),
            4 | 5 => f.list(
                &options.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>(),
                operation == 5,
            ),
            6 => f.plural(option("language"), value).map(str::to_string),
            7 => f
                .direction(value)
                .map(|rtl| if rtl { "rtl" } else { "ltr" }.to_string()),
            _ => Err(bexos_locale_catalog::Error::Unsupported),
        };
        result.map_err(|e| wasmtime::format_err!("locale formatting {e:?}"))
    }
    pub fn encode(&self) -> Result<Vec<u8>, bexos_migration::Error> {
        let mut w = bexos_migration::codec::Encoder::new();
        w.word(1);
        if let Some(c) = &self.client {
            w.word(1);
            w.word(c.descriptor.data);
            w.word(c.descriptor.data_len);
            w.word(c.descriptor.data_generation);
            w.word(c.watcher.map_or(0, |w| w.0));
            w.bytes(c.snapshot());
        } else {
            w.word(0);
        }
        Ok(w.finish())
    }
    pub fn decode(bytes: &[u8], owned: bool) -> Result<Self, bexos_migration::Error> {
        use bexos_migration::{Error, codec::Decoder};
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let client = if r.flag()? {
            let data = r.word()?;
            let data_len = r.word()?;
            let data_generation = r.word()?;
            let watcher = r.word()?;
            let settings = r.bytes(4096)?.to_vec();
            if data == 0 || data_len == 0 || data_len > 64 * 1024 * 1024 {
                return Err(Error::InvalidData);
            }
            Some(
                Client::adopt(
                    LocaleDescriptor {
                        data,
                        data_len,
                        data_generation,
                        settings,
                    },
                    (watcher != 0).then_some(Channel(watcher)),
                    owned,
                )
                .map_err(|_| Error::InvalidData)?,
            )
        } else {
            None
        };
        r.finish()?;
        Ok(Self { client })
    }
    pub fn resources(&self) -> Vec<u64> {
        self.client
            .as_ref()
            .map(|c| {
                core::iter::once(c.descriptor.data)
                    .chain(c.watcher.map(|w| w.0))
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn activate(&mut self) {
        if let Some(c) = &mut self.client {
            c.activate();
        }
    }
}
