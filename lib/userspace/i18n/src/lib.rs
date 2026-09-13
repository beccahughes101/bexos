//! Native locale client; the CLDR VMO stays outside WASM linear memory.
mod mapping;
use bexos_locale_settings::{Settings, encode};
use bexos_userspace::{Channel, Memory, startup::LocaleDescriptor};
use locale_fidl::{FidlDecode, FidlEncode, HandleRef, Status};
pub use mapping::MappedData;
pub const SERVICE: &str = "bexos.locale.LocaleProvider";
pub struct Client {
    pub descriptor: LocaleDescriptor,
    pub settings: Settings,
    pub generation: u64,
    pub watcher: Option<Channel>,
    mapping: Option<MappedData>,
    owns_handles: bool,
}
impl Client {
    /// Takes ownership of the descriptor, including on validation failure.
    pub fn from_descriptor(descriptor: LocaleDescriptor) -> Result<Self, Status> {
        Self::adopt(descriptor, None, true)
    }
    pub fn adopt(
        descriptor: LocaleDescriptor,
        watcher: Option<Channel>,
        owns_handles: bool,
    ) -> Result<Self, Status> {
        let parsed = (|| {
            if descriptor.data == 0
                || descriptor.data_len == 0
                || descriptor.data_len > 64 * 1024 * 1024
            {
                return Err(Status::InvalidArgs);
            }
            let q = locale_fidl::LocaleSnapshot::decode(&descriptor.settings, &[])
                .map_err(|_| Status::InvalidArgs)?;
            let settings = Settings::from_wire(&q.settings).map_err(|_| Status::InvalidArgs)?;
            Ok((settings, q.generation))
        })();
        let (settings, generation) = match parsed {
            Ok(value) => value,
            Err(error) => {
                if owns_handles {
                    let _ = Memory::close(descriptor.data);
                    if let Some(w) = watcher {
                        let _ = Memory::close(w.0);
                    }
                }
                return Err(error);
            }
        };
        Ok(Self {
            descriptor,
            settings,
            generation,
            watcher,
            mapping: None,
            owns_handles,
        })
    }
    pub fn activate(&mut self) {
        self.owns_handles = true;
    }
    pub fn snapshot(&self) -> &[u8] {
        &self.descriptor.settings
    }
    pub fn watch(&mut self, provider: Channel) -> Result<(), Status> {
        if self.watcher.is_some() {
            return Ok(());
        }
        let (local, remote) = Channel::pair().map_err(|_| Status::Io)?;
        let result = (|| {
            let message = call(
                provider,
                3,
                &locale_fidl::LocaleProviderWatchLocaleRequest {
                    listener: HandleRef { raw: remote.0 },
                },
            )?;
            let q = locale_fidl::LocaleProviderWatchLocaleResponse::decode(&message.bytes, &[])
                .map_err(|_| Status::InvalidArgs)?;
            if q.status != Status::Ok {
                return Err(q.status);
            }
            self.replace(q.snapshot)?;
            self.watcher = Some(local);
            Ok(())
        })();
        if result.is_err() {
            let _ = Memory::close(local.0);
        }
        result
    }
    fn replace(&mut self, snapshot: locale_fidl::LocaleSnapshot<'_>) -> Result<bool, Status> {
        if snapshot.generation < self.generation {
            return Ok(false);
        }
        let settings = Settings::from_wire(&snapshot.settings).map_err(|_| Status::InvalidArgs)?;
        let changed = snapshot.generation != self.generation || settings != self.settings;
        let (bytes, _) = encode(&snapshot).map_err(|_| Status::InvalidArgs)?;
        self.generation = snapshot.generation;
        self.settings = settings;
        self.descriptor.settings = bytes;
        Ok(changed)
    }
    pub fn poll(&mut self) -> Result<bool, Status> {
        let Some(watcher) = self.watcher else {
            return Ok(false);
        };
        let mut changed = false;
        for _ in 0..64 {
            let message = match watcher.try_recv() {
                Ok(m) => m,
                Err(kernel_fidl::Status::ErrTimedOut) => break,
                Err(kernel_fidl::Status::ErrPeerClosed) => {
                    let _ = Memory::close(watcher.0);
                    self.watcher = None;
                    return Err(Status::AccessDenied);
                }
                Err(_) => return Err(Status::Io),
            };
            let ordinal = message
                .bytes
                .get(..8)
                .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                .unwrap_or(0);
            if ordinal == 1 && message.handles.is_empty() {
                let q = locale_fidl::LocaleUpdateListenerOnLocaleSettingsChangedRequest::decode(
                    &message.bytes[8..],
                    &[],
                )
                .map_err(|_| Status::InvalidArgs)?;
                changed |= self.replace(q.snapshot)?;
            } else {
                for h in message.handles {
                    let _ = Memory::close(h);
                }
                return Err(Status::InvalidArgs);
            }
        }
        Ok(changed)
    }
    pub fn formatting(&mut self) -> Result<bexos_locale_formatting::Formatting<'_>, Status> {
        if self.mapping.is_none() {
            self.mapping = Some(MappedData::map(
                self.descriptor.data,
                self.descriptor.data_len,
            )?);
        }
        bexos_locale_formatting::Formatting::new(
            self.mapping.as_ref().unwrap().bytes(),
            self.settings.clone(),
        )
        .map_err(|_| Status::InvalidArgs)
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.mapping = None;
        if self.owns_handles {
            let _ = Memory::close(self.descriptor.data);
            if let Some(w) = self.watcher {
                let _ = Memory::close(w.0);
            }
        }
    }
}
pub fn call<T: FidlEncode>(
    provider: Channel,
    ordinal: u64,
    q: &T,
) -> Result<bexos_userspace::Message, Status> {
    let (payload, handles) = encode(q).map_err(|_| Status::InvalidArgs)?;
    let mut bytes = ordinal.to_le_bytes().to_vec();
    bytes.extend(payload);
    if provider.send(&bytes, &handles).is_err() {
        for h in handles {
            let _ = Memory::close(h);
        }
        return Err(Status::Io);
    }
    provider
        .recv_with_timeout(30)
        .map_err(|_| Status::Unavailable)
}

pub mod context;
