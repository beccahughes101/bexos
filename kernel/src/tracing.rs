use crate::state::Global;
use kernel_fidl::Status;

#[derive(Clone, Copy)]
pub struct KernelTraceProducer {
    pub buffer: u64,
    pub mapped_len: u64,
    pub producer_id: u64,
    pub cpu_id: u32,
}

static KERNEL_TRACE: Global<Option<KernelTraceProducer>> = Global::new(None);

pub fn attach(buffer: u64, mapped_len: u64, producer_id: u64, cpu_id: u32) -> Status {
    if buffer == 0
        || mapped_len < (bexos_trace::HEADER_SIZE + bexos_trace::SLOT_SIZE) as u64
        || mapped_len > bexos_trace::MAX_PRODUCER_BUFFER_SIZE as u64
        || producer_id == 0
    {
        return Status::ErrInvalidArgs;
    }
    KERNEL_TRACE.with(|slot| {
        *slot = Some(KernelTraceProducer {
            buffer,
            mapped_len,
            producer_id,
            cpu_id,
        });
    });
    bexos_trace::trace_instant!(bexos_trace::CATEGORY_KERNEL_SCHED, "kernel:trace_attach");
    Status::Ok
}

pub fn detach() -> Result<KernelTraceProducer, Status> {
    let producer = KERNEL_TRACE.with(|slot| slot.take());
    match producer {
        Some(producer) => {
            bexos_trace::trace_instant!(bexos_trace::CATEGORY_KERNEL_SCHED, "kernel:trace_detach");
            Ok(producer)
        }
        None => Err(Status::ErrInvalidHandle),
    }
}

pub fn snapshot() -> Option<KernelTraceProducer> {
    KERNEL_TRACE.with(|slot| *slot)
}
