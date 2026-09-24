//! Child locale handoff uses appd's private authenticated service connection.
use super::*;
use bexos_userspace::startup::LocaleDescriptor;
use locale_fidl::{FidlDecode, FidlEncode, HandleRef};
pub struct Pending(pub Option<LocaleDescriptor>);
impl Pending {
    pub fn sent(&mut self) {
        self.0 = None;
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(d) = self.0.take() {
            let _ = Memory::close(d.data);
        }
    }
}
pub fn prepare(
    registry: &MemoryAppRegistry,
    services: &[state::ManagedService],
    vfsd: Channel,
    legacy: &[state::ComponentConfigRecord],
    package: &str,
    uid: u64,
) -> Result<Pending, String> {
    let provider = services
        .iter()
        .find(|s| s.package == "bexos.service.localed");
    if package != "bexos.service.localed" && provider.is_none() {
        return Ok(Pending(None));
    }
    if package == "bexos.service.localed" {
        // Register the locale schema exactly once as part of bringing up its
        // authoritative provider. Re-registering it synchronously for every
        // application launch contends with per-user preference loads and can
        // leave appd waiting on prefsd before it can even contact localed.
        let record = registry
            .record("bexos.locale.preferences")
            .map_err(|e| format!("locale preferences package {e:?}"))?;
        let root = vfs::get_package_directory(vfsd, &record.archive_id())
            .map_err(|e| format!("locale preferences root {e:?}"))?;
        let registration = (|| {
            let connection = preferences::connect(services)?;
            preferences::register(connection.0, record, root, legacy)
        })();
        let _ = fs::close(root);
        registration.map_err(|e| format!("locale preferences registration {e:?}"))?;
        return Ok(Pending(None));
    }
    let provider = provider.unwrap();
    let (client, server) = Channel::pair().map_err(|e| format!("locale channel {e:?}"))?;
    if let Err(e) = Channel(provider.manager).send(
        b"bexos.locale.LocaleStartup|LocaleStartup|Startup|1||bexos.platform.appd|0|fg",
        &[server.0],
    ) {
        let _ = Memory::close(client.0);
        let _ = Memory::close(server.0);
        return Err(format!("locale binding {e:?}"));
    }
    let connection = preferences::Connection(client);
    let mut bytes = [0; 4096];
    let mut handles = [HandleRef { raw: 0 }; 1];
    let encoded = locale_fidl::LocaleStartupResolveRequest { uid }
        .encode(&mut bytes, &mut handles)
        .map_err(|e| format!("locale request {e:?}"))?;
    let message = Rpc(connection.0)
        .call_raw_with_timeout(1, &bytes[..encoded.bytes], &[], true, 30)
        .map_err(|e| format!("locale resolve {e:?}"))?;
    let mut pending = Pending(None);
    let result = (|| {
        let hs = message
            .handles
            .iter()
            .map(|h| HandleRef { raw: *h })
            .collect::<Vec<_>>();
        let response = locale_fidl::LocaleStartupResolveResponse::decode(&message.bytes, &hs)
            .map_err(|e| format!("locale response {e:?}"))?;
        if response.status != locale_fidl::Status::Ok
            || message.handles.len() != 1
            || response.data.len() != 1
            || response.data_len == 0
            || response.data_len > 64 * 1024 * 1024
        {
            return Err(format!("locale response status {:?}", response.status));
        }
        let n = response
            .snapshot
            .encode(&mut bytes, &mut [])
            .map_err(|e| format!("locale settings {e:?}"))?;
        pending.0 = Some(LocaleDescriptor {
            data: response.data[0].raw,
            data_len: response.data_len,
            data_generation: response.data_generation,
            settings: bytes[..n.bytes].to_vec(),
        });
        Ok(())
    })();
    if result.is_err() {
        for h in message.handles {
            let _ = Memory::close(h);
        }
    }
    result?;
    Ok(pending)
}
