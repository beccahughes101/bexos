use bexos_userspace::{Channel, Memory, Rpc};
use ethernet_fidl::{
    DeviceGetFifoRequest, DeviceGetInfoRequest, DevicePublicClient, DeviceRegisterBufferRequest,
    DeviceStartRequest, HandleRef, Status,
};

pub const RX_BYTES: u64 = 64 * 2048;
pub const TX_BYTES: u64 = 64 * 2048;

#[derive(Clone, Copy)]
pub struct EthernetLink {
    pub control: Channel,
    pub fifo: Channel,
    pub rx_vmo: u64,
    pub tx_vmo: u64,
    pub rx_vmo_id: u32,
    pub tx_vmo_id: u32,
    pub mtu: u32,
    pub mac: [u8; 6],
}

pub fn connect(endpoint: u64) -> Result<EthernetLink, Status> {
    let control = Channel(endpoint);
    let mut client = DevicePublicClient::new(Rpc(control));
    let mut req = [0; 64];
    let mut resp = [0; 256];
    let mut req_handles = [HandleRef { raw: 0 }; 4];
    let mut resp_handles = [HandleRef { raw: 0 }; 4];
    let info = client
        .get_info(
            &DeviceGetInfoRequest {},
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| Status::ErrInvalidArgs)?;
    let rx_vmo = Memory::create(RX_BYTES, 0).map_err(map_kernel_status)?;
    let tx_vmo = Memory::create(TX_BYTES, 0).map_err(map_kernel_status)?;
    // RegisterBuffer transfers its handle. Keep our own references for packet
    // mappings and migration, just as the reconnect path does.
    let rx = register(&mut client, duplicate(rx_vmo)?, RX_BYTES as u32)?;
    let tx = register(&mut client, duplicate(tx_vmo)?, TX_BYTES as u32)?;
    let fifo = client
        .get_fifo(
            &DeviceGetFifoRequest {},
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| Status::ErrInvalidArgs)?;
    if fifo.status != Status::Ok {
        return Err(fifo.status);
    }
    let start = client
        .start(
            &DeviceStartRequest {},
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| Status::ErrInvalidArgs)?;
    if start.status != Status::Ok {
        return Err(start.status);
    }
    Ok(EthernetLink {
        control,
        fifo: Channel(fifo.fifo_handle.raw),
        rx_vmo,
        tx_vmo,
        rx_vmo_id: rx,
        tx_vmo_id: tx,
        mtu: info.info.mtu,
        mac: info.info.mac.octets,
    })
}

pub fn reconnect(resources: crate::link::LinkResources) -> Result<EthernetLink, Status> {
    let control = Channel(resources.control);
    let mut client = DevicePublicClient::new(Rpc(control));
    let mut req = [0; 64];
    let mut resp = [0; 256];
    let mut req_handles = [HandleRef { raw: 0 }; 4];
    let mut resp_handles = [HandleRef { raw: 0 }; 4];
    let info = client
        .get_info(
            &DeviceGetInfoRequest {},
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| Status::ErrInvalidArgs)?;
    let rx = register(&mut client, duplicate(resources.rx_vmo)?, RX_BYTES as u32)?;
    let tx = register(&mut client, duplicate(resources.tx_vmo)?, TX_BYTES as u32)?;
    let fifo = client
        .get_fifo(
            &DeviceGetFifoRequest {},
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| Status::ErrInvalidArgs)?;
    if fifo.status != Status::Ok {
        return Err(fifo.status);
    }
    let start = client
        .start(
            &DeviceStartRequest {},
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| Status::ErrInvalidArgs)?;
    if start.status != Status::Ok {
        return Err(start.status);
    }
    Ok(EthernetLink {
        control,
        fifo: Channel(fifo.fifo_handle.raw),
        rx_vmo: resources.rx_vmo,
        tx_vmo: resources.tx_vmo,
        rx_vmo_id: rx,
        tx_vmo_id: tx,
        mtu: info.info.mtu,
        mac: info.info.mac.octets,
    })
}

fn duplicate(handle: u64) -> Result<u64, Status> {
    Memory::duplicate(handle, 1 | 2 | 4 | 16 | 32).map_err(map_kernel_status)
}

fn register(
    client: &mut DevicePublicClient<Rpc>,
    vmo: u64,
    size_bytes: u32,
) -> Result<u32, Status> {
    let mut req = [0; 64];
    let mut resp = [0; 64];
    let mut req_handles = [HandleRef { raw: 0 }; 4];
    let mut resp_handles = [HandleRef { raw: 0 }; 4];
    let registered = client
        .register_buffer(
            &DeviceRegisterBufferRequest {
                vmo: HandleRef { raw: vmo },
                size_bytes,
            },
            &mut req,
            &mut req_handles,
            &mut resp,
            &mut resp_handles,
        )
        .map_err(|_| Status::ErrInvalidArgs)?;
    if registered.status == Status::Ok {
        Ok(registered.vmo_id)
    } else {
        Err(registered.status)
    }
}

fn map_kernel_status(status: kernel_fidl::Status) -> Status {
    match status {
        kernel_fidl::Status::ErrInvalidHandle => Status::ErrInvalidHandle,
        kernel_fidl::Status::ErrAccessDenied => Status::ErrAccessDenied,
        kernel_fidl::Status::ErrNoMemory => Status::ErrNoMemory,
        kernel_fidl::Status::ErrBufferTooSmall => Status::ErrBufferTooSmall,
        kernel_fidl::Status::ErrPeerClosed => Status::ErrPeerClosed,
        kernel_fidl::Status::ErrTimedOut => Status::ErrTimedOut,
        _ => Status::ErrInvalidArgs,
    }
}
