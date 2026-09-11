use alloc::string::{String, ToString};
use alloc::vec;
use bexos_userspace::{Channel, Memory, TraceProducerDescriptor, log};
use tracing_fidl::{
    FidlDecode, FidlEncode, HandleRef, TraceCategory, TraceRegistryRegisterProducerRequest,
    TraceRegistryRegisterProducerResponse,
};

const TRACE_REGISTRY_METHODS: &[u64] = &[1, 2, 3];
const TRACE_REGISTRY_RIGHTS: u32 = 1 | 2 | 4 | 16 | 32;

pub struct TraceProducerAllocation {
    pub startup: TraceProducerDescriptor,
    registry_buffer: u64,
}

pub struct PendingTraceProducer {
    pub producer_id: u64,
    pub pid: u64,
    pub main_tid: u64,
    pub process_name: String,
    pub mapped_len: u64,
    pub categories: u32,
    pub buffer: u64,
}

impl PendingTraceProducer {
    pub fn from_allocation(process_name: &str, allocation: &TraceProducerAllocation) -> Self {
        Self {
            producer_id: allocation.startup.producer_id,
            pid: allocation.startup.pid,
            main_tid: allocation.startup.main_tid,
            process_name: process_name.to_string(),
            mapped_len: allocation.startup.mapped_len,
            categories: bexos_trace::CATEGORY_ALL,
            buffer: allocation.registry_buffer,
        }
    }
}

pub fn allocate_trace_producer(
    pid: u64,
    main_tid: u64,
) -> Result<TraceProducerAllocation, kernel_fidl::Status> {
    let buffer = Memory::create(bexos_trace::MAX_PRODUCER_BUFFER_SIZE as u64, 0)?;
    let registry_buffer = match Memory::duplicate(buffer, TRACE_REGISTRY_RIGHTS) {
        Ok(duplicate) => duplicate,
        Err(error) => {
            let _ = Memory::close(buffer);
            return Err(error);
        }
    };
    Ok(TraceProducerAllocation {
        startup: TraceProducerDescriptor {
            buffer,
            mapped_len: bexos_trace::MAX_PRODUCER_BUFFER_SIZE as u64,
            producer_id: pid,
            pid,
            main_tid,
        },
        registry_buffer,
    })
}

pub fn register_now(traced: Option<Channel>, pending: PendingTraceProducer) -> bool {
    let Some(traced) = traced else {
        let _ = Memory::close(pending.buffer);
        return false;
    };
    let registry = match open_registry(traced) {
        Ok(channel) => channel,
        Err(_) => {
            let _ = Memory::close(pending.buffer);
            return false;
        }
    };
    match send_register(registry, pending) {
        Ok(()) => true,
        Err(()) => false,
    }
}

fn open_registry(traced: Channel) -> Result<Channel, kernel_fidl::Status> {
    let (client, provider) = Channel::pair()?;
    let methods = join_ordinals(TRACE_REGISTRY_METHODS);
    let metadata =
        alloc::format!("bexos.tracing.TraceRegistry|TraceRegistry|SystemPrivileged|{methods}|");
    traced.send(metadata.as_bytes(), &[provider.0])?;
    Ok(client)
}

fn send_register(registry: Channel, pending: PendingTraceProducer) -> Result<(), ()> {
    let mut bytes = vec![0; 1024];
    let mut handles = [HandleRef { raw: 0 }; 1];
    let encoded = TraceRegistryRegisterProducerRequest {
        producer_id: pending.producer_id,
        pid: pending.pid,
        main_tid: pending.main_tid,
        process_name: &pending.process_name,
        categories: TraceCategory(pending.categories),
        mapped_len: pending.mapped_len,
        buffer: HandleRef {
            raw: pending.buffer,
        },
    }
    .encode(&mut bytes, &mut handles)
    .map_err(|_| {
        let _ = Memory::close(pending.buffer);
    })?;
    let raw_handles = [handles[0].raw];
    if registry
        .send(&ordinal_message(1, &bytes[..encoded.bytes]), &raw_handles)
        .is_err()
    {
        let _ = Memory::close(pending.buffer);
        return Err(());
    }
    let message = registry.recv_blocking().map_err(|_| ())?;
    let refs = message
        .handles
        .iter()
        .map(|handle| HandleRef { raw: *handle })
        .collect::<alloc::vec::Vec<_>>();
    let response =
        TraceRegistryRegisterProducerResponse::decode(&message.bytes, &refs).map_err(|_| ())?;
    if response.status == kernel_fidl::Status::Ok {
        Ok(())
    } else {
        log(&alloc::format!(
            "appd: traced registration failed producer={} status={:?}\n",
            pending.producer_id,
            response.status
        ));
        Err(())
    }
}

fn ordinal_message(ordinal: u64, payload: &[u8]) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&ordinal.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn join_ordinals(method_ordinals: &[u64]) -> String {
    let mut out = String::new();
    for (index, ordinal) in method_ordinals.iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        out.push_str(&ordinal.to_string());
    }
    out
}
