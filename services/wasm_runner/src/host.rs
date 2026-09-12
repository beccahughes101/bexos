use bexos_userspace::{Channel, Memory, preferences as pref_rpc};
use bexos_wasm_runtime::{
    host::Host,
    resources::{Entry, Handle, Kind, READ, TRANSFER, WRITE},
    wasi::{
        filesystem::FsResult,
        sockets::NetResult,
        wasi::{
            filesystem::types as fs,
            sockets::{
                network as net,
                tcp::ShutdownType,
                udp::{IncomingDatagram, OutgoingDatagram},
            },
        },
    },
};
use preferences_fidl::{
    FidlDecode, HandleRef as PreferencesHandleRef, Status, ThemeManagerWatchThemeRequest,
    ThemeManagerWatchThemeResponse, ThemeObserverOnThemeChangedRequest,
};
use std::sync::{Arc, Mutex};
use wasmtime::{Result, bail};
mod filesystem;
mod network;
mod trust;
pub(crate) mod ui;
mod ui_renderer;
pub struct NativeHandle {
    pub raw: u64,
    pub kind: Kind,
    pub rights: u32,
    pub companions: Vec<u64>,
    pub allowed_methods: Option<Vec<u64>>,
    pub grant: Option<bexos_wasm_runtime::resources::Grant>,
    pub ownership: Option<Arc<std::sync::atomic::AtomicBool>>,
}
impl Handle for NativeHandle {
    fn grant(&self) -> Option<&bexos_wasm_runtime::resources::Grant> {
        self.grant.as_ref()
    }
    fn companions(&self) -> &[u64] {
        &self.companions
    }
    fn allowed_methods(&self) -> Option<&[u64]> {
        self.allowed_methods.as_deref()
    }
    fn native(&self) -> u64 {
        self.raw
    }
    fn kind(&self) -> Kind {
        self.kind
    }
    fn rights(&self) -> u32 {
        self.rights
    }
}
impl Drop for NativeHandle {
    fn drop(&mut self) {
        if self
            .ownership
            .as_ref()
            .is_some_and(|v| !v.load(std::sync::atomic::Ordering::Acquire))
        {
            return;
        }
        let _ = Memory::close(self.raw);
        for h in &self.companions {
            let _ = Memory::close(*h);
        }
    }
}
pub fn entry(name: impl Into<String>, raw: u64, kind: Kind, rights: u32) -> Entry {
    Entry {
        name: name.into(),
        handle: Arc::new(NativeHandle {
            raw,
            kind,
            rights,
            companions: Vec::new(),
            allowed_methods: None,
            grant: None,
            ownership: None,
        }),
    }
}
pub struct NativeHost {
    pub trust: Mutex<Option<std::sync::Weak<dyn Handle>>>,
    pub theme: Mutex<ThemeState>,
    pub fs_lock: Mutex<()>,
    pub ui: Mutex<ui::UiState>,
}
pub struct ThemeState {
    manager: Option<Arc<dyn Handle>>,
    observer: Option<Channel>,
    current: bexos_ui_runtime::RuntimeTheme,
}
impl NativeHost {
    pub fn new() -> Self {
        Self {
            trust: Mutex::new(None),
            theme: Mutex::new(ThemeState::default()),
            fs_lock: Mutex::new(()),
            ui: Mutex::new(ui::UiState::new()),
        }
    }
    pub fn observe_grant(&self, entry: &Entry) {
        if entry.name == "bexos.security.trust.AppTrustManager" {
            *self.trust.lock().unwrap() = Some(Arc::downgrade(&entry.handle));
        }
        if entry.name == "bexos.ui.theme.ThemeManager" {
            self.theme.lock().unwrap().observe(&*entry.handle);
        }
    }
    pub fn replace_grants<'a>(&self, entries: impl Iterator<Item = &'a Entry>) {
        *self.trust.lock().unwrap() = None;
        *self.theme.lock().unwrap() = ThemeState::default();
        for entry in entries {
            self.observe_grant(entry);
        }
    }

    pub fn ui_quiescent(&self) -> bool {
        self.ui.lock().unwrap().quiescent()
    }

    fn refresh_theme(&self) -> bool {
        self.theme.lock().unwrap().poll()
    }
}

impl Default for ThemeState {
    fn default() -> Self {
        Self {
            manager: None,
            observer: None,
            current: bexos_ui_runtime::RuntimeTheme::default(),
        }
    }
}

impl ThemeState {
    fn observe(&mut self, handle: &dyn Handle) {
        self.manager = Some(clone_handle(handle));
    }

    fn current(&self) -> bexos_ui_runtime::RuntimeTheme {
        self.current.clone()
    }

    fn poll(&mut self) -> bool {
        if self.observer.is_none() && self.manager.is_some() {
            let _ = self.watch();
        }
        let Some(observer) = self.observer else {
            return false;
        };
        let mut changed = false;
        loop {
            let message = match observer.try_recv() {
                Ok(message) => message,
                Err(kernel_fidl::Status::ErrPeerClosed) => {
                    self.observer = None;
                    break;
                }
                Err(_) => break,
            };
            let handles = pref_rpc::refs(&message);
            if message.bytes.get(..8) != Some(&1u64.to_le_bytes()[..]) {
                for handle in message.handles {
                    let _ = Memory::close(handle);
                }
                continue;
            }
            let Ok(request) =
                ThemeObserverOnThemeChangedRequest::decode(&message.bytes[8..], &handles)
            else {
                for handle in message.handles {
                    let _ = Memory::close(handle);
                }
                continue;
            };
            if let Ok(css) = pref_rpc::read_vmo(request.stylesheet.raw, request.stylesheet_len) {
                self.current = theme_from_parts(
                    request.metadata.generation,
                    request.metadata.color_scheme,
                    request.metadata.text_scale_percent,
                    request.metadata.reduce_motion,
                    css,
                );
                changed = true;
            }
        }
        changed
    }

    fn watch(&mut self) -> Result<()> {
        let Some(manager) = self.manager.clone() else {
            return Ok(());
        };
        let (client, server) =
            Channel::pair().map_err(|error| wasmtime::format_err!("theme observer: {error:?}"))?;
        let request = ThemeManagerWatchThemeRequest {
            observer: PreferencesHandleRef { raw: server.0 },
        };
        let message = pref_rpc::call(Channel(manager.native()), 2, &request)
            .map_err(|error| wasmtime::format_err!("theme watch: {error:?}"))?;
        let handles = pref_rpc::refs(&message);
        let response = ThemeManagerWatchThemeResponse::decode(&message.bytes, &handles)
            .map_err(|_| wasmtime::format_err!("theme watch response"))?;
        if response.status != Status::Ok {
            let _ = Memory::close(client.0);
            return Ok(());
        }
        let Some(stylesheet) = response.stylesheet.first() else {
            let _ = Memory::close(client.0);
            return Ok(());
        };
        let css = pref_rpc::read_vmo(stylesheet.raw, response.stylesheet_len)
            .map_err(|error| wasmtime::format_err!("theme stylesheet: {error:?}"))?;
        self.current = theme_from_parts(
            response.metadata.generation,
            response.metadata.color_scheme,
            response.metadata.text_scale_percent,
            response.metadata.reduce_motion,
            css,
        );
        self.observer = Some(client);
        Ok(())
    }
}

fn theme_from_parts(
    generation: u64,
    color_scheme: &str,
    text_scale_percent: u32,
    reduce_motion: bool,
    css: Vec<u8>,
) -> bexos_ui_runtime::RuntimeTheme {
    let mut preferences = bexos_ui_theme::ThemePreferences {
        color_scheme: bexos_ui_theme::ColorScheme::parse(color_scheme)
            .unwrap_or(bexos_ui_theme::ColorScheme::Dark),
        text_scale_percent: text_scale_percent.clamp(75, 300),
        reduce_motion,
        custom_css: String::new(),
    };
    let user_css = String::from_utf8(css).unwrap_or_else(|_| preferences.user_css());
    if user_css.is_empty() {
        preferences.custom_css.clear();
    }
    bexos_ui_runtime::RuntimeTheme {
        generation,
        preferences,
        user_css,
    }
}

fn clone_handle(handle: &dyn Handle) -> Arc<dyn Handle> {
    Arc::new(NativeHandle {
        raw: handle.native(),
        kind: handle.kind(),
        rights: handle.rights(),
        companions: handle.companions().to_vec(),
        allowed_methods: handle.allowed_methods().map(|methods| methods.to_vec()),
        grant: handle.grant().cloned(),
        ownership: Some(Arc::new(std::sync::atomic::AtomicBool::new(false))),
    })
}
impl Host for NativeHost {
    fn channel_pair(&self) -> Result<(Arc<dyn Handle>, Arc<dyn Handle>)> {
        let (a, b) = Channel::pair().map_err(|e| wasmtime::format_err!("channel pair: {e:?}"))?;
        Ok((
            entry("", a.0, Kind::Channel, READ | WRITE | TRANSFER).handle,
            entry("", b.0, Kind::Channel, READ | WRITE | TRANSFER).handle,
        ))
    }
    fn duplicate_handle(&self, h: &dyn Handle) -> Result<Arc<dyn Handle>> {
        if !h.companions().is_empty() {
            bail!("resource needs typed duplication");
        }
        let (_, rights) = Memory::object_info(h.native())
            .map_err(|e| wasmtime::format_err!("duplicate metadata: {e:?}"))?;
        let raw = Memory::duplicate(h.native(), rights)
            .map_err(|e| wasmtime::format_err!("duplicate: {e:?}"))?;
        Ok(Arc::new(NativeHandle {
            raw,
            kind: h.kind(),
            rights: h.rights(),
            companions: Vec::new(),
            allowed_methods: h.allowed_methods().map(|m| m.to_vec()),
            grant: h.grant().cloned(),
            ownership: None,
        }))
    }

    fn random(&self, length: usize) -> Result<Vec<u8>> {
        bexos_userspace::random::bytes(length)
            .map_err(|e| wasmtime::format_err!("kernel entropy: {e:?}"))
    }
    fn monotonic_ns(&self) -> u64 {
        let ticks = bexos_userspace::syscall::ticks();
        let hz = bexos_userspace::syscall::frequency();
        ((u128::from(ticks) * 1_000_000_000) / u128::from(hz.max(1))) as u64
    }
    fn wall_clock_ns(&self) -> Result<u64> {
        bexos_userspace::clock::realtime_ns()
            .map_err(|e| wasmtime::format_err!("realtime clock: {e:?}"))
    }
    fn log(&self, bytes: &[u8]) {
        bexos_userspace::log(&String::from_utf8_lossy(bytes));
    }
    fn channel_write(&self, channel: &dyn Handle, bytes: &[u8], handles: &[Entry]) -> Result<()> {
        // Raw channel transfer has no metadata envelope. Never strip a service's
        // method policy or a socket's control endpoint from a delegated resource.
        if handles.iter().any(|e| {
            e.handle.grant().is_some()
                || e.handle.allowed_methods().is_some()
                || !e.handle.companions().is_empty()
        }) {
            bail!("resource requires typed delegation");
        }
        let mut duplicated = Vec::new();
        for e in handles {
            let rights = 1
                | if e.handle.rights() & READ != 0 { 2 } else { 0 }
                | if e.handle.rights() & WRITE != 0 { 4 } else { 0 };
            let native_rights = match Memory::object_info(e.handle.native()) {
                Ok((_, rights)) => rights,
                Err(e) => {
                    for h in duplicated {
                        let _ = Memory::close(h);
                    }
                    bail!("transfer rights: {e:?}");
                }
            };
            let rights = rights | (native_rights & 32);
            match Memory::duplicate(e.handle.native(), rights) {
                Ok(h) => duplicated.push(h),
                Err(e) => {
                    for h in duplicated {
                        let _ = Memory::close(h);
                    }
                    bail!("handle attenuation: {e:?}");
                }
            }
        }
        if let Err(e) = Channel(channel.native()).send(bytes, &duplicated) {
            for h in duplicated {
                let _ = Memory::close(h);
            }
            bail!("channel write: {e:?}");
        }
        Ok(())
    }
    fn channel_read(
        &self,
        channel: &dyn Handle,
        max_bytes: usize,
        max_handles: usize,
    ) -> Result<(Vec<u8>, Vec<Arc<dyn Handle>>)> {
        let m = Channel(channel.native())
            .try_recv_bounded(max_bytes, max_handles)
            .map_err(|e| match e {
                kernel_fidl::Status::ErrTimedOut => {
                    wasmtime::Error::from(std::io::Error::from(std::io::ErrorKind::WouldBlock))
                }
                kernel_fidl::Status::ErrPeerClosed => {
                    wasmtime::Error::from(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
                }
                _ => wasmtime::format_err!("channel read: {e:?}"),
            })?;
        if let Some(methods) = channel.allowed_methods() {
            let ordinal = m
                .bytes
                .get(..8)
                .map(|b| u64::from_le_bytes(b.try_into().unwrap()));
            if !ordinal.is_some_and(|n| methods.contains(&n)) {
                for h in m.handles {
                    let _ = Memory::close(h);
                }
                bail!("service method denied");
            }
        }
        // The kernel, rather than a guest-provided tag, establishes received types
        // and rights. Higher-level filesystem/service grants still need typed adapters.
        let mut handles = Vec::new();
        for (index, raw) in m.handles.iter().copied().enumerate() {
            let (kind, rights) = match Memory::object_info(raw) {
                Ok(info) => info,
                Err(e) => {
                    for h in &m.handles[index..] {
                        let _ = Memory::close(*h);
                    }
                    bail!("received handle info: {e:?}");
                }
            };
            let kind = match kind {
                kernel_fidl::ObjectType::Channel => Kind::Channel,
                kernel_fidl::ObjectType::Socket => Kind::Socket,
                _ => Kind::Opaque,
            };
            let rights = (if rights & 1 != 0 { TRANSFER } else { 0 })
                | (if rights & 2 != 0 { READ } else { 0 })
                | (if rights & 4 != 0 { WRITE } else { 0 });
            handles.push(entry("", raw, kind, rights).handle);
        }
        Ok((m.bytes, handles))
    }

    fn verify_signature(&self, module: &[u8], envelope: &[u8]) -> Result<()> {
        let handle = self
            .trust
            .lock()
            .unwrap()
            .as_ref()
            .and_then(std::sync::Weak::upgrade);
        trust::verify(handle.as_ref().map(|h| h.native()), module, envelope)
    }
    fn ui_create_view(
        &self,
        flatland: &dyn Handle,
        display: Option<&dyn Handle>,
        width: u32,
        height: u32,
    ) -> Result<u32> {
        self.ui
            .lock()
            .unwrap()
            .create_view(flatland, display, width, height)
    }
    fn ui_configure_view(&self, view: u32, width: u32, height: u32, scale: f32) -> Result<()> {
        self.ui
            .lock()
            .unwrap()
            .configure_view(view, width, height, scale)
    }
    fn ui_register_asset(&self, view: u32, asset: u32, kind: u32, bytes: &[u8]) -> Result<()> {
        self.ui
            .lock()
            .unwrap()
            .register_asset(view, asset, kind, bytes)
    }
    fn ui_release_asset(&self, view: u32, asset: u32) -> Result<()> {
        self.ui.lock().unwrap().release_asset(view, asset)
    }
    fn ui_set_node_scene(&self, view: u32, node: u64, batch: &[u8]) -> Result<()> {
        self.ui.lock().unwrap().set_node_scene(view, node, batch)
    }
    fn ui_submit_scene(&self, view: u32, batch: &[u8]) -> Result<()> {
        self.ui.lock().unwrap().submit_scene(view, batch)
    }
    fn ui_submit_document(&self, view: u32, document: &[u8]) -> Result<()> {
        self.refresh_theme();
        let theme = self.theme.lock().unwrap().current();
        self.ui
            .lock()
            .unwrap()
            .submit_document(view, document, &theme)
    }
    fn ui_poll_input(&self, view: u32) -> Result<Vec<bexos_wasm_runtime::host::UiInputEvent>> {
        if self.refresh_theme() {
            let theme = self.theme.lock().unwrap().current();
            self.ui.lock().unwrap().redraw_documents(&theme)?;
        }
        self.ui.lock().unwrap().poll_input(view)
    }
    fn ui_presentation_status(
        &self,
        view: u32,
    ) -> Result<bexos_wasm_runtime::host::UiPresentationStatus> {
        if self.refresh_theme() {
            let theme = self.theme.lock().unwrap().current();
            self.ui.lock().unwrap().redraw_documents(&theme)?;
        }
        self.ui.lock().unwrap().presentation_status(view)
    }
    fn ui_active_backend(&self, view: u32) -> Result<bexos_wasm_runtime::host::UiBackendStatus> {
        self.ui.lock().unwrap().active_backend(view)
    }
    fn ui_close_view(&self, view: u32) -> Result<()> {
        self.ui.lock().unwrap().close_view(view)
    }
    fn read_file(&self, h: &dyn Handle, offset: u64, len: usize) -> Result<Vec<u8>> {
        let _guard = self.fs_lock.lock().unwrap();
        filesystem::read(h, offset, len)
    }
    fn write_file(&self, h: &dyn Handle, offset: u64, bytes: &[u8]) -> Result<usize> {
        let _guard = self.fs_lock.lock().unwrap();
        filesystem::write(h, offset, bytes)
    }
    fn open_file(
        &self,
        h: &dyn Handle,
        path: &str,
        open: fs::OpenFlags,
        flags: fs::DescriptorFlags,
    ) -> FsResult<Entry> {
        filesystem::open(h, path, open, flags)
    }
    fn stat_file(&self, h: &dyn Handle) -> FsResult<fs::DescriptorStat> {
        filesystem::stat(h)
    }
    fn read_directory(&self, h: &dyn Handle) -> FsResult<Vec<fs::DirectoryEntry>> {
        filesystem::read_directory(h)
    }
    fn sync_file(&self, h: &dyn Handle) -> FsResult<()> {
        filesystem::sync(h)
    }
    fn resize_file(&self, h: &dyn Handle, size: u64) -> FsResult<()> {
        filesystem::resize(h, size)
    }
    fn unlink_file(&self, h: &dyn Handle, path: &str, directory: bool) -> FsResult<()> {
        filesystem::unlink(h, path, directory)
    }
    fn socket_pair(&self) -> Result<(Arc<dyn Handle>, Arc<dyn Handle>)> {
        let (a, b) = bexos_userspace::Socket::pair()
            .map_err(|e| wasmtime::format_err!("socket pair: {e:?}"))?;
        Ok((
            entry("", a.0, Kind::Socket, READ | WRITE | TRANSFER).handle,
            entry("", b.0, Kind::Socket, READ | WRITE | TRANSFER).handle,
        ))
    }
    fn socket_half_close(&self, h: &dyn Handle, read: bool, write: bool) -> Result<()> {
        bexos_userspace::Socket(h.native())
            .shutdown(read, write)
            .map_err(|e| wasmtime::format_err!("socket shutdown: {e:?}"))
    }
    fn socket_read(&self, h: &dyn Handle, length: usize) -> Result<Vec<u8>> {
        match bexos_userspace::Socket(h.native()).read(length as u32) {
            Ok(bytes) => Ok(bytes),
            Err(kernel_fidl::Status::ErrPeerClosed) => Ok(Vec::new()),
            Err(e) => Err(wasmtime::format_err!("socket read: {e:?}")),
        }
    }
    fn socket_write(&self, h: &dyn Handle, bytes: &[u8]) -> Result<usize> {
        bexos_userspace::Socket(h.native())
            .write(bytes)
            .or_else(|e| {
                if e == kernel_fidl::Status::ErrNoMemory {
                    Ok(0)
                } else {
                    Err(e)
                }
            })
            .map(|n| n as usize)
            .map_err(|e| wasmtime::format_err!("socket write: {e:?}"))
    }
    fn network_ready(&self, handle: &dyn Handle, udp: bool, write: bool) -> NetResult<bool> {
        network::ready(handle, udp, write)
    }
    fn socket_ready(&self, h: &dyn Handle, write: bool) -> Result<bool> {
        let i = bexos_userspace::Socket(h.native())
            .info()
            .map_err(|e| wasmtime::format_err!("socket readiness: {e:?}"))?;
        Ok(if write {
            i.local_write_closed
                || i.peer_read_closed
                || bexos_userspace::Socket(h.native())
                    .wait_io(true, 0)
                    .map_err(|e| wasmtime::format_err!("socket writable: {e:?}"))?
        } else {
            i.readable_bytes > 0 || i.peer_write_closed || i.local_read_closed
        })
    }
    fn resolve_addresses(&self, g: &dyn Handle, name: &str) -> NetResult<Vec<net::IpAddress>> {
        network::resolve(g, name)
    }
    fn connect_tcp(
        &self,
        g: &dyn Handle,
        a: &net::IpSocketAddress,
    ) -> NetResult<(Entry, net::IpSocketAddress)> {
        network::connect(g, a)
    }
    fn listen_tcp(&self, g: &dyn Handle, a: &net::IpSocketAddress) -> NetResult<Entry> {
        network::listen(g, a)
    }
    fn accept_tcp(&self, g: &dyn Handle) -> NetResult<(Entry, net::IpSocketAddress)> {
        network::accept(g)
    }
    fn shutdown_socket(&self, h: &dyn Handle, how: ShutdownType) -> NetResult<()> {
        network::shutdown(h, how)
    }
    fn bind_udp(&self, g: &dyn Handle, a: &net::IpSocketAddress) -> NetResult<Entry> {
        network::bind_udp(g, a)
    }
    fn receive_udp(
        &self,
        h: &dyn Handle,
        n: usize,
        remote: Option<&net::IpSocketAddress>,
    ) -> NetResult<Vec<IncomingDatagram>> {
        network::receive_udp(h, n, remote)
    }
    fn send_udp(
        &self,
        h: &dyn Handle,
        d: &[OutgoingDatagram],
        remote: Option<&net::IpSocketAddress>,
    ) -> NetResult<u64> {
        network::send_udp(h, d, remote)
    }
}

pub fn grant_metadata(
    binding: &bexos_userspace::service_binding::ServiceBinding,
) -> bexos_wasm_runtime::resources::Grant {
    bexos_wasm_runtime::resources::Grant {
        service: binding.service.clone(),
        protocol: binding.protocol.clone(),
        capability: binding.capability.clone(),
        method_ordinals: binding.method_ordinals.clone(),
        permission_values: binding.permission_values.clone(),
        caller_package: binding.caller_package.clone(),
        caller_uid: binding.caller_uid,
        caller_foreground: binding.caller_foreground,
    }
}
