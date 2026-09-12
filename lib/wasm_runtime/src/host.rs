use crate::resources::{Entry, Handle};
use crate::wasi::{
    sockets::NetResult,
    wasi::sockets::{
        network::{ErrorCode, IpAddress, IpSocketAddress},
        tcp::ShutdownType,
        udp::{IncomingDatagram, OutgoingDatagram},
    },
};
use std::sync::Arc;
use wasmtime::Result;
/// Every platform operation is supplied explicitly by the embedding process.
/// Resource handles passed here have already been checked by the guest table.
pub trait Host: Send + Sync {
    fn channel_pair(&self) -> Result<(Arc<dyn Handle>, Arc<dyn Handle>)> {
        wasmtime::bail!("channel creation unavailable")
    }
    fn duplicate_handle(&self, _handle: &dyn Handle) -> Result<Arc<dyn Handle>> {
        wasmtime::bail!("handle duplication unavailable")
    }

    fn verify_signature(&self, _module: &[u8], _signature: &[u8]) -> Result<()> {
        wasmtime::bail!("signature verification unavailable")
    }
    fn open_file(
        &self,
        _directory: &dyn Handle,
        _path: &str,
        _open: crate::wasi::wasi::filesystem::types::OpenFlags,
        _flags: crate::wasi::wasi::filesystem::types::DescriptorFlags,
    ) -> crate::wasi::filesystem::FsResult<Entry> {
        Err(crate::wasi::wasi::filesystem::types::ErrorCode::Unsupported)
    }
    fn stat_file(
        &self,
        _handle: &dyn Handle,
    ) -> crate::wasi::filesystem::FsResult<crate::wasi::wasi::filesystem::types::DescriptorStat>
    {
        Err(crate::wasi::wasi::filesystem::types::ErrorCode::Unsupported)
    }
    fn read_directory(
        &self,
        _handle: &dyn Handle,
    ) -> crate::wasi::filesystem::FsResult<Vec<crate::wasi::wasi::filesystem::types::DirectoryEntry>>
    {
        Err(crate::wasi::wasi::filesystem::types::ErrorCode::Unsupported)
    }
    fn sync_file(&self, _handle: &dyn Handle) -> crate::wasi::filesystem::FsResult<()> {
        Err(crate::wasi::wasi::filesystem::types::ErrorCode::Unsupported)
    }
    fn resize_file(
        &self,
        _handle: &dyn Handle,
        _size: u64,
    ) -> crate::wasi::filesystem::FsResult<()> {
        Err(crate::wasi::wasi::filesystem::types::ErrorCode::Unsupported)
    }
    fn unlink_file(
        &self,
        _handle: &dyn Handle,
        _path: &str,
        _directory: bool,
    ) -> crate::wasi::filesystem::FsResult<()> {
        Err(crate::wasi::wasi::filesystem::types::ErrorCode::Unsupported)
    }
    fn resolve_addresses(&self, _grant: &dyn Handle, _name: &str) -> NetResult<Vec<IpAddress>> {
        Err(ErrorCode::NotSupported)
    }
    fn connect_tcp(
        &self,
        _grant: &dyn Handle,
        _address: &IpSocketAddress,
    ) -> NetResult<(Entry, IpSocketAddress)> {
        Err(ErrorCode::NotSupported)
    }
    fn listen_tcp(&self, _grant: &dyn Handle, _address: &IpSocketAddress) -> NetResult<Entry> {
        Err(ErrorCode::NotSupported)
    }
    fn accept_tcp(&self, _listener: &dyn Handle) -> NetResult<(Entry, IpSocketAddress)> {
        Err(ErrorCode::NotSupported)
    }
    fn shutdown_socket(&self, _socket: &dyn Handle, _how: ShutdownType) -> NetResult<()> {
        Err(ErrorCode::NotSupported)
    }
    fn bind_udp(&self, _grant: &dyn Handle, _address: &IpSocketAddress) -> NetResult<Entry> {
        Err(ErrorCode::NotSupported)
    }
    fn receive_udp(
        &self,
        _socket: &dyn Handle,
        _max: usize,
        _remote: Option<&IpSocketAddress>,
    ) -> NetResult<Vec<IncomingDatagram>> {
        Err(ErrorCode::NotSupported)
    }
    fn send_udp(
        &self,
        _socket: &dyn Handle,
        _datagrams: &[OutgoingDatagram],
        _remote: Option<&IpSocketAddress>,
    ) -> NetResult<u64> {
        Err(ErrorCode::NotSupported)
    }
    fn wall_clock_ns(&self) -> Result<u64> {
        wasmtime::bail!("wall clock unavailable")
    }
    fn random(&self, _length: usize) -> Result<Vec<u8>> {
        wasmtime::bail!("secure randomness unavailable")
    }
    fn read_file(&self, _handle: &dyn Handle, _offset: u64, _length: usize) -> Result<Vec<u8>> {
        wasmtime::bail!("filesystem unavailable")
    }
    fn write_file(&self, _handle: &dyn Handle, _offset: u64, _bytes: &[u8]) -> Result<usize> {
        wasmtime::bail!("filesystem unavailable")
    }
    fn socket_pair(&self) -> Result<(Arc<dyn Handle>, Arc<dyn Handle>)> {
        wasmtime::bail!("socket pair unavailable")
    }
    fn socket_half_close(&self, _handle: &dyn Handle, _read: bool, _write: bool) -> Result<()> {
        wasmtime::bail!("socket half-close unavailable")
    }
    fn socket_read(&self, _handle: &dyn Handle, _length: usize) -> Result<Vec<u8>> {
        wasmtime::bail!("socket unavailable")
    }
    fn socket_write(&self, _handle: &dyn Handle, _bytes: &[u8]) -> Result<usize> {
        wasmtime::bail!("socket unavailable")
    }
    fn network_ready(&self, _handle: &dyn Handle, _udp: bool, _write: bool) -> NetResult<bool> {
        Err(ErrorCode::NotSupported)
    }
    fn socket_ready(&self, _handle: &dyn Handle, _write: bool) -> Result<bool> {
        wasmtime::bail!("socket unavailable")
    }
    fn monotonic_ns(&self) -> u64;
    fn log(&self, bytes: &[u8]);
    fn channel_write(&self, channel: &dyn Handle, bytes: &[u8], handles: &[Entry]) -> Result<()>;
    fn channel_read(
        &self,
        channel: &dyn Handle,
        max_bytes: usize,
        max_handles: usize,
    ) -> Result<(Vec<u8>, Vec<Arc<dyn Handle>>)>;
    fn ui_create_view(
        &self,
        _flatland: &dyn Handle,
        _display: Option<&dyn Handle>,
        _width: u32,
        _height: u32,
    ) -> Result<u32> {
        wasmtime::bail!("ui unavailable")
    }
    fn ui_configure_view(&self, _view: u32, _width: u32, _height: u32, _scale: f32) -> Result<()> {
        wasmtime::bail!("ui unavailable")
    }
    fn ui_register_asset(&self, _view: u32, _asset: u32, _kind: u32, _bytes: &[u8]) -> Result<()> {
        wasmtime::bail!("ui unavailable")
    }
    fn ui_release_asset(&self, _view: u32, _asset: u32) -> Result<()> {
        wasmtime::bail!("ui unavailable")
    }
    fn ui_set_node_scene(&self, _view: u32, _node: u64, _batch: &[u8]) -> Result<()> {
        wasmtime::bail!("node scenes unavailable")
    }
    fn ui_submit_scene(&self, _view: u32, _batch: &[u8]) -> Result<()> {
        wasmtime::bail!("ui unavailable")
    }
    fn ui_submit_document(&self, _view: u32, _document: &[u8]) -> Result<()> {
        wasmtime::bail!("ui document unavailable")
    }
    fn ui_poll_input(&self, _view: u32) -> Result<Vec<UiInputEvent>> {
        wasmtime::bail!("ui unavailable")
    }
    fn ui_presentation_status(&self, _view: u32) -> Result<UiPresentationStatus> {
        wasmtime::bail!("ui unavailable")
    }
    fn ui_active_backend(&self, _view: u32) -> Result<UiBackendStatus> {
        wasmtime::bail!("ui unavailable")
    }
    fn ui_close_view(&self, _view: u32) -> Result<()> {
        wasmtime::bail!("ui unavailable")
    }
}

#[derive(Clone, Debug, Default)]
pub struct UiInputEvent {
    pub kind: u32,
    pub device: u64,
    pub id: u32,
    pub phase: u32,
    pub x: f64,
    pub y: f64,
    pub buttons: u32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub code: u32,
    pub key_state: u32,
    pub modifiers: u32,
    pub unicode: u32,
}

#[derive(Clone, Debug, Default)]
pub struct UiPresentationStatus {
    pub accepted_sequence: u64,
    pub pending_count: u32,
    pub latched_time_ticks: u64,
    pub scene_generation: u64,
}

#[derive(Clone, Debug, Default)]
pub struct UiBackendStatus {
    pub backend: String,
    pub failure: Option<String>,
}
