use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use bexos_tee_driver_client::*;
use bexos_userspace::Memory;

use crate::ql_proto::{
    QL_BUFFER_SIZE, QL_HEADER_SIZE, QL_OP_CONNECT, QL_OP_DISCONNECT, QL_OP_GET_EVENT, QL_OP_RECV,
    QL_OP_SEND, encode_command, keymint_response_chunk, response_payload,
};
use crate::{kernel_status, log_status, secure_monitor};

const SMC_SC_CREATE_QL_TIPC_DEV: u64 = 0x3200_001e;
const SMC_SC_SHUTDOWN_QL_TIPC_DEV: u64 = 0x3200_001f;
const SMC_SC_HANDLE_QL_TIPC_DEV_CMD: u64 = 0x3200_0020;
const SMC_SC_RESTART_LAST: u64 = 0x3c00_0000;
const SMC_SC_RESTART_FIQ: u64 = 0x3c00_0002;
const SMC_SC_NOP: u64 = 0x3c00_0003;
const SM_ERR_INTERRUPTED: i32 = -3;
const SM_ERR_BUSY: i32 = -5;
const SM_ERR_FIQ_INTERRUPTED: i32 = -12;
const SM_ERR_CPU_IDLE: i32 = -13;
const SM_ERR_NOP_INTERRUPTED: i32 = -14;
const SM_ERR_NOP_DONE: i32 = -15;
const NS_PTE_MAIR_SHIFT: u64 = 48;
const NS_PTE_SHAREABLE_SHIFT: u64 = 8;
// User VMOs are mapped as normal write-back, read/write-allocate memory by the
// kernel. The legacy Trusty memory-object ABI requires the NS descriptor to
// describe that same mapping; claiming uncached memory here would permit the
// two worlds to observe incoherent copies of the command page.
const NS_MAIR_NORMAL_CACHED_WB_RWA: u64 = 0xff;
const NS_INNER_SHAREABLE: u64 = 0x3;
const MAX_RESTARTS: usize = 4096;
const MAX_CONNECT_ATTEMPTS: usize = 64;
const MAX_EVENT_POLLS: usize = 256;
const IPC_HANDLE_POLL_READY: u32 = 0x1;
const IPC_HANDLE_POLL_ERROR: u32 = 0x2;
const IPC_HANDLE_POLL_HUP: u32 = 0x4;
const IPC_HANDLE_POLL_MSG: u32 = 0x8;
const IPC_HANDLE_POLL_SEND_UNBLOCKED: u32 = 0x10;

#[derive(Debug)]
pub struct QueuedTipc {
    vmo: u64,
    va: u64,
    pin: u64,
    pa: u64,
    #[cfg(target_arch = "x86_64")]
    monitor_handle: u64,
    next_cookie: u64,
    pub storage_handler: Option<StorageProxyHandler>,
    storage_handle: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct TipcSession {
    pub id: u64,
    pub uuid: [u8; 16],
    pub handle: u32,
    pub port: String,
}

impl QueuedTipc {
    pub fn create() -> Result<Self, i32> {
        let vmo = Memory::create(QL_BUFFER_SIZE as u64, 0)
            .map_err(kernel_status)
            .inspect_err(|status| log_status("QL-TIPC VMO create", *status))?;
        let va = match Memory::map(vmo, QL_BUFFER_SIZE as u64, 2 | 4) {
            Ok(va) => va,
            Err(status) => {
                log_status("QL-TIPC VMO map", kernel_status(status));
                let _ = Memory::close(vmo);
                return Err(kernel_status(status));
            }
        };
        let (pa, pin) = match Memory::pin(vmo) {
            Ok(pin) => pin,
            Err(status) => {
                log_status("QL-TIPC VMO pin", kernel_status(status));
                let _ = Memory::unmap(va, QL_BUFFER_SIZE as u64);
                let _ = Memory::close(vmo);
                return Err(kernel_status(status));
            }
        };
        let mut tipc = Self {
            vmo,
            va,
            pin,
            pa,
            #[cfg(target_arch = "x86_64")]
            monitor_handle: 0,
            next_cookie: 1,
            storage_handler: None,
            storage_handle: None,
        };
        #[cfg(target_arch = "x86_64")]
        {
            tipc.monitor_handle = crate::ql_monitor::register(pa, QL_BUFFER_SIZE as u64)?;
        }
        if let Err(status) = tipc.call_create() {
            log_status("QL-TIPC device SMC", status);
            let _ = tipc.shutdown();
            return Err(status);
        }
        Ok(tipc)
    }

    pub fn reset(&mut self) -> Result<(), i32> {
        self.shutdown()?;
        self.storage_handle = None;
        self.call_create()
    }

    pub fn connect(&mut self, id: u64, uuid: [u8; 16], port: &str) -> Result<TipcSession, i32> {
        if port.is_empty() || port.len() > 128 {
            return Err(STATUS_INVALID_ARGS);
        }
        let cookie = self.alloc_cookie();
        let mut payload = Vec::new();
        payload.extend_from_slice(&cookie.to_le_bytes());
        payload.extend_from_slice(&0u64.to_le_bytes());
        payload.extend_from_slice(port.as_bytes());
        payload.push(0);
        for _ in 0..MAX_CONNECT_ATTEMPTS {
            self.command(QL_OP_CONNECT, 0, &payload)?;
            let (header, _) = self.response(QL_OP_CONNECT)?;
            if header.status == 0 && header.handle != 0 {
                self.wait_for_event(header.handle, cookie, IPC_HANDLE_POLL_READY)?;
                if port == "com.android.trusty.storage.proxy" {
                    self.storage_handle = Some(header.handle);
                }
                return Ok(TipcSession {
                    id,
                    uuid,
                    handle: header.handle,
                    port: port.into(),
                });
            }
            // A manifest start-port may launch its deferred TA on the first
            // connection attempt before that TA has created the runtime port.
            // Re-enter the secure scheduler and retry for a bounded interval;
            // permanent absence and authorization failures still fail closed.
            self.pump_storage()?;
            drive_secure_work(false)?;
        }
        Err(STATUS_NOT_FOUND)
    }

    pub fn close(&mut self, handle: u32) -> Result<(), i32> {
        if self.storage_handle == Some(handle) {
            self.storage_handle = None;
        }
        self.command(QL_OP_DISCONNECT, handle, &[])?;
        let (header, _) = self.response(QL_OP_DISCONNECT)?;
        if header.status == 0 {
            Ok(())
        } else {
            Err(STATUS_PEER_CLOSED)
        }
    }

    pub fn invoke(&mut self, handle: u32, request: &[u8], out: &mut [u8]) -> Result<usize, i32> {
        self.invoke_framed(handle, request, out, false)
    }

    pub fn invoke_keymint(
        &mut self,
        handle: u32,
        request: &[u8],
        out: &mut [u8],
    ) -> Result<usize, i32> {
        self.invoke_framed(handle, request, out, true)
    }

    pub fn try_recv(&mut self, handle: u32, out: &mut [u8]) -> Result<usize, i32> {
        self.command(QL_OP_GET_EVENT, handle, &0u64.to_le_bytes())?;
        let (event_header, event_payload) = self.response(QL_OP_GET_EVENT)?;
        if event_header.status != 0 {
            return Err(STATUS_PEER_CLOSED);
        }
        let Some(event) = TipcEvent::decode(event_payload) else {
            return Err(STATUS_UNAVAILABLE);
        };
        if event.event == 0 {
            drive_secure_work(false)?;
            return Err(STATUS_TIMED_OUT);
        }
        if event.event & (IPC_HANDLE_POLL_ERROR | IPC_HANDLE_POLL_HUP) != 0 {
            return Err(STATUS_PEER_CLOSED);
        }
        if event.handle != handle || event.event & IPC_HANDLE_POLL_MSG == 0 {
            return Err(STATUS_TIMED_OUT);
        }
        self.command(QL_OP_RECV, handle, &[])?;
        let (recv_header, payload) = self.response(QL_OP_RECV)?;
        if recv_header.status != 0 {
            return Err(STATUS_PEER_CLOSED);
        }
        if payload.len() > out.len() {
            return Err(STATUS_BUFFER_TOO_SMALL);
        }
        out[..payload.len()].copy_from_slice(payload);
        Ok(payload.len())
    }

    pub fn send(&mut self, handle: u32, bytes: &[u8]) -> Result<(), i32> {
        self.command(QL_OP_SEND, handle, bytes)?;
        let (header, _) = self.response(QL_OP_SEND)?;
        if header.status == 0 {
            Ok(())
        } else {
            Err(STATUS_PEER_CLOSED)
        }
    }

    pub fn pump_storage(&mut self) -> Result<(), i32> {
        let (Some(handle), Some(handler)) = (self.storage_handle, self.storage_handler) else {
            return Ok(());
        };
        let mut request = [0u8; 8192];
        let count = match self.try_recv(handle, &mut request) {
            Ok(count) => count,
            Err(STATUS_TIMED_OUT) => return Ok(()),
            Err(status) => return Err(status),
        };
        let mut response = [0u8; 8192];
        let mut written = 0;
        let status = unsafe {
            (handler.dispatch)(
                handler.context,
                request.as_ptr(),
                count,
                response.as_mut_ptr(),
                response.len(),
                &mut written,
            )
        };
        if status != STATUS_OK {
            return Err(status);
        }
        if written > response.len() {
            return Err(STATUS_BUFFER_TOO_SMALL);
        }
        self.send(handle, &response[..written])
    }

    fn invoke_framed(
        &mut self,
        handle: u32,
        request: &[u8],
        out: &mut [u8],
        keymint_chunks: bool,
    ) -> Result<usize, i32> {
        bexos_userspace::syscall::log("tee-driver-trusty: QL-TIPC send begin\n");
        self.command(QL_OP_SEND, handle, request)?;
        bexos_userspace::syscall::log("tee-driver-trusty: QL-TIPC send returned\n");
        let (send_header, _) = self.response(QL_OP_SEND)?;
        if send_header.status != 0 {
            return Err(STATUS_PEER_CLOSED);
        }
        let mut written = 0usize;
        loop {
            bexos_userspace::syscall::log("tee-driver-trusty: QL-TIPC response wait begin\n");
            self.wait_for_event(handle, 0, IPC_HANDLE_POLL_MSG)?;
            bexos_userspace::syscall::log("tee-driver-trusty: QL-TIPC response ready\n");
            self.command(QL_OP_RECV, handle, &[])?;
            let (recv_header, payload) = self.response(QL_OP_RECV)?;
            if recv_header.status != 0 {
                return Err(STATUS_PEER_CLOSED);
            }
            let (more, content) = if keymint_chunks {
                keymint_response_chunk(payload).map_err(|_| STATUS_UNAVAILABLE)?
            } else {
                (false, payload)
            };
            let end = written
                .checked_add(content.len())
                .ok_or(STATUS_BUFFER_TOO_SMALL)?;
            if end > out.len() {
                return Err(STATUS_BUFFER_TOO_SMALL);
            }
            out[written..end].copy_from_slice(content);
            written = end;
            if !more {
                return Ok(written);
            }
        }
    }

    fn alloc_cookie(&mut self) -> u64 {
        let cookie = self.next_cookie;
        self.next_cookie = self.next_cookie.saturating_add(1).max(1);
        cookie
    }

    fn buffer_mut(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.va as *mut u8, QL_BUFFER_SIZE) }
    }

    fn buffer(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.va as *const u8, QL_BUFFER_SIZE) }
    }

    fn call_create(&mut self) -> Result<(), i32> {
        let regs = call_with_mem_id(
            SMC_SC_CREATE_QL_TIPC_DEV,
            self.shared_mem_id(),
            QL_BUFFER_SIZE as u64,
        )?;
        if regs[0] == 0 {
            Ok(())
        } else {
            Err(STATUS_UNAVAILABLE)
        }
    }

    fn shutdown(&mut self) -> Result<(), i32> {
        let regs = call_with_mem_id(
            SMC_SC_SHUTDOWN_QL_TIPC_DEV,
            self.shared_mem_id(),
            QL_BUFFER_SIZE as u64,
        )?;
        if regs[0] == 0 {
            // Closing the QL device queues peer-disconnect events in Trusty.
            // Run those handlers before another device can reconnect the
            // single-owner storage proxy; SMC completion alone does not mean
            // the storage TA has cleared its previous IPC handle.
            drive_secure_work(false)
        } else {
            Err(STATUS_UNAVAILABLE)
        }
    }

    fn command(&mut self, opcode: u16, handle: u32, payload: &[u8]) -> Result<(), i32> {
        encode_command(self.buffer_mut(), opcode, handle, payload).map_err(
            |error| match error {
                crate::ql_proto::Error::PayloadTooLarge => STATUS_BUFFER_TOO_SMALL,
                crate::ql_proto::Error::BufferTooSmall
                | crate::ql_proto::Error::InvalidResponse => STATUS_INVALID_ARGS,
            },
        )?;
        let command_len = QL_HEADER_SIZE
            .checked_add(payload.len())
            .ok_or(STATUS_BUFFER_TOO_SMALL)? as u64;
        let regs = call_with_mem_id(
            SMC_SC_HANDLE_QL_TIPC_DEV_CMD,
            self.shared_mem_id(),
            command_len,
        )?;
        if (regs[0] as i32) >= 0 {
            Ok(())
        } else {
            Err(STATUS_UNAVAILABLE)
        }
    }

    fn response(&self, opcode: u16) -> Result<(crate::ql_proto::Header, &[u8]), i32> {
        response_payload(self.buffer(), opcode).map_err(|_| STATUS_UNAVAILABLE)
    }

    fn shared_mem_id(&self) -> u64 {
        #[cfg(target_arch = "x86_64")]
        {
            self.monitor_handle
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            (self.pa & 0xffff_ffff_f000)
                | (NS_MAIR_NORMAL_CACHED_WB_RWA << NS_PTE_MAIR_SHIFT)
                | (NS_INNER_SHAREABLE << NS_PTE_SHAREABLE_SHIFT)
        }
    }

    fn wait_for_event(&mut self, handle: u32, cookie: u64, expected: u32) -> Result<(), i32> {
        for poll in 0..MAX_EVENT_POLLS {
            self.pump_storage()?;
            self.command(QL_OP_GET_EVENT, handle, &0u64.to_le_bytes())?;
            let (header, payload) = self.response(QL_OP_GET_EVENT)?;
            if header.status != 0 {
                return Err(STATUS_PEER_CLOSED);
            }
            let Some(event) = TipcEvent::decode(payload) else {
                return Err(STATUS_UNAVAILABLE);
            };
            if poll < 4 {
                bexos_userspace::syscall::log(&format!(
                    "tee-driver-trusty: QL-TIPC poll={poll} event={:#x} handle={} cookie={}\n",
                    event.event, event.handle, event.cookie
                ));
            }
            if event.event == 0 {
                drive_secure_work(poll < 4)?;
                continue;
            }
            if event.event & (IPC_HANDLE_POLL_ERROR | IPC_HANDLE_POLL_HUP) != 0 {
                return Err(STATUS_PEER_CLOSED);
            }
            if event.handle == handle
                && (cookie == 0 || event.cookie == cookie)
                && event.event & expected != 0
            {
                return Ok(());
            }
            if expected == IPC_HANDLE_POLL_SEND_UNBLOCKED
                && event.handle == handle
                && event.event & IPC_HANDLE_POLL_SEND_UNBLOCKED != 0
            {
                return Ok(());
            }
            drive_secure_work(false)?;
        }
        Err(STATUS_TIMED_OUT)
    }
}

#[derive(Clone, Copy)]
struct TipcEvent {
    event: u32,
    handle: u32,
    cookie: u64,
}

impl TipcEvent {
    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 {
            return None;
        }
        Some(Self {
            event: u32::from_le_bytes(bytes[0..4].try_into().ok()?),
            handle: u32::from_le_bytes(bytes[4..8].try_into().ok()?),
            cookie: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
        })
    }
}

impl Drop for QueuedTipc {
    fn drop(&mut self) {
        let _ = self.shutdown();
        #[cfg(target_arch = "x86_64")]
        crate::ql_monitor::unregister(self.monitor_handle);
        let _ = Memory::unpin(self.pin);
        let _ = Memory::unmap(self.va, QL_BUFFER_SIZE as u64);
        let _ = Memory::close(self.vmo);
    }
}

fn call_with_mem_id(fid: u64, mem_id: u64, size: u64) -> Result<[u64; 8], i32> {
    #[cfg(target_arch = "x86_64")]
    {
        crate::ql_monitor::call(fid, mem_id, size)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        call_with_restarts(fid, mem_id as u32 as u64, mem_id >> 32, size)
    }
}

fn call_with_restarts(x0: u64, x1: u64, x2: u64, x3: u64) -> Result<[u64; 8], i32> {
    let mut call = [x0, x1, x2, x3];
    let mut restart_count = 0;
    loop {
        let regs = secure_monitor(call[0], call[1], call[2], call[3], 0, 0, 0, 0)?;
        match regs[0] as i32 {
            0 => return Ok(regs),
            SM_ERR_INTERRUPTED => call = [SMC_SC_RESTART_LAST, 0, 0, 0],
            SM_ERR_CPU_IDLE => {
                // The stdcall remains outstanding. Match Trusty's reference
                // client: yield the normal CPU, then resume that exact call.
                // A concurrent NOP here can report idle while the stdcall is
                // still blocked and starve its eventual completion.
                bexos_userspace::yield_now();
                call = [SMC_SC_RESTART_LAST, 0, 0, 0];
            }
            SM_ERR_FIQ_INTERRUPTED => {
                call = [SMC_SC_RESTART_FIQ, 0, 0, 0];
            }
            SM_ERR_NOP_INTERRUPTED => {
                call = [SMC_SC_NOP, 0, 0, 0];
            }
            SM_ERR_BUSY => {
                // Retry whichever operation is current. Before a stdcall has
                // been accepted this is the original command; after an
                // interruption it is RESTART_LAST.
                bexos_userspace::yield_now();
            }
            _ => return Ok(regs),
        }
        restart_count += 1;
        if restart_count > MAX_RESTARTS {
            return Err(STATUS_TIMED_OUT);
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn drive_secure_work(_trace: bool) -> Result<(), i32> {
    // Trusty's monitor vCPU runs independently; no ARM restart SMC is needed.
    bexos_userspace::yield_now();
    Ok(())
}

#[cfg(not(target_arch = "x86_64"))]
fn drive_secure_work(trace: bool) -> Result<(), i32> {
    let mut call = SMC_SC_NOP;
    for attempt in 0..MAX_RESTARTS {
        let regs = secure_monitor(call, 0, 0, 0, 0, 0, 0, 0)?;
        if trace && attempt < 4 {
            bexos_userspace::syscall::log(&format!(
                "tee-driver-trusty: secure-work attempt={attempt} call={call:#x} result={}\n",
                regs[0] as i32
            ));
        }
        match regs[0] as i32 {
            SM_ERR_NOP_DONE | 0 => return Ok(()),
            SM_ERR_NOP_INTERRUPTED => call = SMC_SC_NOP,
            SM_ERR_FIQ_INTERRUPTED => call = SMC_SC_RESTART_FIQ,
            SM_ERR_CPU_IDLE | SM_ERR_INTERRUPTED => {
                bexos_userspace::yield_now();
                call = SMC_SC_RESTART_LAST;
            }
            SM_ERR_BUSY => {
                bexos_userspace::yield_now();
            }
            _ => return Err(STATUS_UNAVAILABLE),
        }
    }
    Err(STATUS_TIMED_OUT)
}

pub fn endpoint_port(uuid: [u8; 16], request: &TeeEndpoint) -> Option<String> {
    if request.port_len != 0 {
        if request.port_ptr.is_null() || request.port_len > 128 {
            return None;
        }
        let bytes = unsafe { core::slice::from_raw_parts(request.port_ptr, request.port_len) };
        return core::str::from_utf8(bytes).ok().map(String::from);
    }
    builtin_port(uuid).map(String::from)
}

fn builtin_port(uuid: [u8; 16]) -> Option<&'static str> {
    match uuid {
        crate::KEYMINT_UUID => Some("com.android.trusty.keymint"),
        crate::GATEKEEPER_UUID => Some("com.android.trusty.gatekeeper"),
        crate::AVB_UUID => Some("com.android.trusty.avb"),
        crate::ORCHESTRATOR_UUID => Some("com.bexos.orchestrator"),
        crate::AUTHMGR_BE_UUID => Some("com.android.trusty.rust.authmgr.V1"),
        crate::STORAGE_UUID => Some("com.android.trusty.storage.proxy"),
        _ => None,
    }
}
