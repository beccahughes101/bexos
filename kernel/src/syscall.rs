use crate::arch::ArchAPI;
use crate::mmu::PhysicalBackend;
use alloc::vec;
use alloc::vec::Vec;
use bexos_kernel_core::runtime::Context;
use bexos_kernel_core::runtime::{Backend, Runtime};
use bexos_kernel_core::sched::{
    DeadlineProfile as CoreDeadlineProfile, FairProfile as CoreFairProfile,
    SchedulingProfile as CoreSchedulingProfile,
};
use core::sync::atomic::{AtomicUsize, Ordering};
use kernel_fidl::*;
pub(crate) type Rt = Runtime<PhysicalBackend>;

static TRUSTY_SMC_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn dispatch(frame: *mut Context, number: u32) {
    bexos_trace::trace_counter!(
        bexos_trace::CATEGORY_IPC_MESSAGES,
        "kernel:syscall_number",
        number as i64,
    );
    let f = unsafe { &mut *frame };
    let syscall_protocol = f.syscall_words_mut()[0];
    let syscall_ordinal = f.syscall_words_mut()[1];
    crate::userspace::RUNTIME.with(|s| {
        if let Some(rt) = s.as_mut() {
            crate::memory::reclamation::maintain(rt);
            let _ = rt.bind_current_cpu(crate::arch::CurrentArch::current_cpu_id() as u8);
            rt.scheduler
                .account_runtime(crate::arch::CurrentArch::monotonic_ns());
        }
    });
    match number {
        1 => crate::userspace::RUNTIME.with(|s| {
            let rt = s.as_mut().unwrap();
            let caller = rt.current_thread as u64 + 1;
            let r = raw(rt, f.syscall_words());
            rt.scheduler.account_executing(
                crate::arch::CurrentArch::current_cpu_id() as u8,
                caller,
                crate::arch::CurrentArch::monotonic_ns(),
            );
            match r {
                Ok((b, h)) => {
                    f.syscall_words_mut()[0] = 0;
                    f.syscall_words_mut()[1] = b as u64;
                    f.syscall_words_mut()[2] = h as u64;
                }
                Err(e) => {
                    f.syscall_words_mut()[0] = e as i32 as u64;
                    f.syscall_words_mut()[1] = 0;
                    f.syscall_words_mut()[2] = 0;
                }
            }
        }),
        2 => crate::sched::yield_now(frame),
        3 => crate::userspace::RUNTIME.with(|s| {
            let rt = s.as_mut().unwrap();
            let len = f.syscall_words_mut()[1] as usize;
            if len <= 4096 {
                let mut bytes = vec![0; len];
                if rt
                    .copy_from_user(f.syscall_words_mut()[0], &mut bytes)
                    .is_ok()
                {
                    if let Ok(text) = core::str::from_utf8(&bytes) {
                        use core::fmt::Write;
                        crate::state::UART.with(|u| {
                            if let Some(u) = u.as_mut() {
                                let _ = u.write_str(text);
                            }
                        });
                    }
                }
            }
        }),
        4 => {
            crate::userspace::RUNTIME.with(|s| {
                let rt = s.as_ref().unwrap();
                crate::log_line(&alloc::format!(
                    "kernel: process exit package={}",
                    rt.processes[rt.current].package
                ));
            });
            crate::userspace::RUNTIME.with(|s| {
                let rt = s.as_mut().unwrap();
                rt.scheduler
                    .account_runtime(crate::arch::CurrentArch::monotonic_ns());
                rt.exit_current();
            });
            crate::sched::timer_tick(frame);
        }
        5 => {
            f.syscall_words_mut()[0] = 0;
            crate::transplant::commit_pending(frame);
        }
        6 => crate::userspace::RUNTIME.with(|s| {
            let rt = s.as_ref().unwrap();
            f.syscall_words_mut()[0] = rt.heap_vmar_handle().unwrap_or(0);
        }),
        7 => f.syscall_words_mut()[0] = crate::arch::CurrentArch::monotonic_ns(),
        8 => {
            if f.syscall_words_mut()[0] < bexos_boot::USER_END {
                f.set_thread_pointer(f.syscall_words()[0]);
                f.syscall_words_mut()[0] = 0;
            } else {
                f.syscall_words_mut()[0] = Status::ErrInvalidArgs as i32 as u64;
            }
        }
        9 => f.syscall_words_mut()[0] = f.thread_pointer(),
        10 => f.syscall_words_mut()[0] = crate::arch::CurrentArch::pci_ecam_base(),
        11 => crate::userspace::RUNTIME.with(|s| {
            let rt = s.as_ref().unwrap();
            let process = &rt.processes[rt.current];
            f.syscall_words_mut()[0] = if process.package == "bexos.driver.rtc.cmos"
                && process.hardware == 2
                && !process.quarantined
                && f.syscall_words_mut()[0] < 128
                && f.syscall_words_mut()[1] <= 511
            {
                crate::arch::CurrentArch::cmos(
                    f.syscall_words_mut()[0] as u8,
                    (f.syscall_words_mut()[1] & 256 != 0).then_some(f.syscall_words_mut()[1] as u8),
                )
                .map(u64::from)
                .unwrap_or(Status::ErrInvalidArgs as i32 as u64)
            } else {
                Status::ErrAccessDenied as i32 as u64
            };
        }),
        12 => crate::userspace::RUNTIME.with(|s| {
            let rt = s.as_mut().unwrap();
            let process = &rt.processes[rt.current];
            let result = if process.package != "bexos.driver.debugd" || process.quarantined {
                Err(Status::ErrAccessDenied)
            } else if f.syscall_words()[1] > 128 * 1024 {
                Err(Status::ErrBufferTooSmall)
            } else {
                let mut bytes = vec![0; f.syscall_words()[1] as usize];
                rt.copy_from_user(f.syscall_words()[0], &mut bytes)
                    .and_then(|_| {
                        // Trap entry masks local interrupts; this shared console
                        // lock also serializes writers on other CPUs. Do not add
                        // newline conversion to a binary protocol frame.
                        crate::state::UART.with(|uart| {
                            let uart = uart.as_mut().ok_or(Status::ErrInvalidArgs)?;
                            for byte in bytes {
                                uart.write_byte_poll(byte);
                            }
                            Ok(())
                        })
                    })
            };
            f.syscall_words_mut()[0] = result.err().map_or(0, |status| status as i32 as u64);
        }),
        _ => f.syscall_words_mut()[0] = Status::ErrInvalidArgs as i32 as u64,
    }
    let should_reschedule = number == 1
        && crate::userspace::RUNTIME.with(|s| {
            let rt = s.as_ref().unwrap();
            !rt.current_thread_can_return()
        })
        || (number == 1 && method(syscall_protocol, syscall_ordinal) == Some("YieldThread"));
    if should_reschedule {
        // Blocking calls update scheduler ownership before this dispatcher
        // selects the successor. Preserve the outgoing syscall-return frame
        // while `current_thread` still identifies the caller; otherwise a
        // later wake resumes the caller from its previous saved PC/registers.
        crate::userspace::RUNTIME.with(|s| s.as_mut().unwrap().save_context(unsafe { *frame }));
        crate::sched::timer_tick(frame);
    }
    crate::userspace::RUNTIME.with(|s| {
        let rt = s.as_mut().unwrap();
        rt.save_context(unsafe { *frame });
        if rt.has_pending_reclamation() {
            crate::arch::CurrentArch::program_scheduler_deadline(
                rt.next_deadline_on_cpu(crate::arch::CurrentArch::current_cpu_id() as u8),
            );
        }
    });
    crate::transplant::live_step();
}
fn raw(rt: &mut Rt, r: &[u64; 10]) -> Result<(usize, usize), Status> {
    bexos_trace::trace_scope!(bexos_trace::CATEGORY_IPC_MESSAGES, "kernel:fidl_syscall");
    let (n, k, cap, hcap) = (r[3] as usize, r[5] as usize, r[7] as usize, r[9] as usize);
    if n > 65536 || cap > 65536 || k > 64 || hcap > 64 {
        return Err(Status::ErrBufferTooSmall);
    }
    if !rt.valid_range(rt.current, r[6], cap as u64, 4)
        || !rt.valid_range(rt.current, r[8], (hcap * 8) as u64, 4)
    {
        return Err(Status::ErrAccessDenied);
    }
    let mut req = vec![0; n];
    if let Err(status) = rt.copy_from_user(r[2], &mut req) {
        if r[0] == 9 {
            crate::log_line(&alloc::format!(
                "kernel: secure-monitor request copy failed status={status:?} ptr={:#x} len={n}",
                r[2]
            ));
        }
        return Err(status);
    }
    let mut hb = vec![0; k * 8];
    if let Err(status) = rt.copy_from_user(r[4], &mut hb) {
        if r[0] == 9 {
            crate::log_line(&alloc::format!(
                "kernel: secure-monitor handle copy failed status={status:?} ptr={:#x} count={k}",
                r[4]
            ));
        }
        return Err(status);
    }
    let hs: Vec<_> = hb
        .chunks_exact(8)
        .map(|b| HandleRef {
            raw: u64::from_le_bytes(b.try_into().unwrap()),
        })
        .collect();
    for h in &hs {
        if h.raw != 0 {
            rt.capability(h.raw, 0)?;
        }
    }
    let mut out = vec![0; cap];
    let mut oh = vec![HandleRef { raw: 0 }; hcap];
    let e =
        route(rt, r[0], r[1], &req, &hs, &mut out, &mut oh).map_err(|_| Status::ErrInvalidArgs)?;
    if let Err(status) = rt.copy_to_user(r[6], &out[..e.bytes]) {
        if r[0] == 9 {
            crate::log_line(&alloc::format!(
                "kernel: secure-monitor response copy failed status={status:?} ptr={:#x} len={}",
                r[6],
                e.bytes
            ));
        }
        return Err(status);
    }
    let mut handles = Vec::new();
    for h in &oh[..e.handles] {
        handles.extend_from_slice(&h.raw.to_le_bytes());
    }
    if let Err(status) = rt.copy_to_user(r[8], &handles) {
        if r[0] == 9 {
            crate::log_line(&alloc::format!(
                "kernel: secure-monitor response-handle copy failed status={status:?} ptr={:#x} count={}",
                r[8],
                e.handles
            ));
        }
        return Err(status);
    }
    Ok((e.bytes, e.handles))
}
fn method(protocol: u64, ordinal: u64) -> Option<&'static str> {
    bexos_kernel_core::kernel_services::routing::syscall_method(protocol, ordinal)
}

const fn q_channel_flow_id(ordinal: u64) -> u64 {
    0x1_0000_0000 | ordinal
}

fn route(
    rt: &mut Rt,
    protocol: u64,
    ordinal: u64,
    req: &[u8],
    hs: &[HandleRef],
    out: &mut [u8],
    oh: &mut [HandleRef],
) -> Result<EncodeResult, FidlWireError> {
    if protocol == 7 {
        return crate::migration::dispatch(rt, ordinal, req, hs, out, oh);
    }
    macro_rules! decode {
        ($t:ty) => {
            <$t>::decode(req, hs)?
        };
    }
    macro_rules! reply {
        ($v:expr) => {
            $v.encode(out, oh)
        };
    }
    fn status<T>(r: &Result<T, Status>) -> Status {
        match r {
            Ok(_) => Status::Ok,
            Err(e) => *e,
        }
    }
    fn from_fidl_profile(info: SchedulingProfileInfo) -> CoreSchedulingProfile {
        match info {
            SchedulingProfileInfo::Fair(fair) => CoreSchedulingProfile::Fair(CoreFairProfile {
                priority: fair.priority,
                weight: fair.weight,
            }),
            SchedulingProfileInfo::Deadline(deadline) => {
                CoreSchedulingProfile::Deadline(CoreDeadlineProfile {
                    capacity_ns: deadline.capacity_ns,
                    deadline_ns: deadline.deadline_ns,
                    period_ns: deadline.period_ns,
                })
            }
        }
    }
    let h = |raw| HandleRef { raw };
    match (protocol, method(protocol, ordinal)) {
        (1, Some("CreateChannel")) => {
            let (a, b) = rt.create_channel();
            reply!(ChannelControlCreateChannelResponse {
                status: Status::Ok,
                local_endpoint: h(a),
                remote_endpoint: h(b)
            })
        }
        (1, Some("WriteMessage")) => {
            bexos_trace::trace_instant!(bexos_trace::CATEGORY_IPC_MESSAGES, "kernel:channel_send");
            let q = decode!(ChannelControlWriteMessageRequest);
            bexos_trace::trace_counter!(
                bexos_trace::CATEGORY_IPC_MESSAGES,
                "kernel:channel_send_bytes",
                q.data.len() as i64
            );
            let handles: Vec<_> = q.handles.iter().map(|v| v.raw).collect();
            let r = rt.write_message(q.channel.raw, q.data, &handles);
            reply!(ChannelControlWriteMessageResponse { status: status(&r) })
        }
        (1, Some("ReadMessage")) => {
            bexos_trace::trace_instant!(bexos_trace::CATEGORY_IPC_MESSAGES, "kernel:channel_read");
            let q = decode!(ChannelControlReadMessageRequest);
            let r = rt.read_message(q.channel.raw, q.max_bytes as usize, q.max_handles as usize);
            let st = status(&r);
            let (bytes, handles) = match r {
                Ok(m) => (m.bytes, m.handles),
                Err(_) => (Vec::new(), Vec::new()),
            };
            let refs: Vec<_> = handles.iter().map(|v| h(*v)).collect();
            reply!(ChannelControlReadMessageResponse {
                status: st,
                data: &bytes,
                handles: &refs
            })
        }
        (1, Some("SetPolicy")) => {
            let q = decode!(ChannelControlSetPolicyRequest);
            let r = rt.set_channel_policy(
                q.channel.raw,
                q.enable_priority_inheritance,
                q.enable_timeslice_donation,
            );
            reply!(ChannelControlSetPolicyResponse { status: status(&r) })
        }
        (1, Some("Call")) => {
            bexos_trace::trace_flow_begin!(
                bexos_trace::CATEGORY_IPC_MESSAGES,
                "kernel:channel_call",
                q_channel_flow_id(ordinal)
            );
            let q = decode!(ChannelControlCallRequest);
            bexos_trace::trace_counter!(
                bexos_trace::CATEGORY_IPC_MESSAGES,
                "kernel:channel_call_bytes",
                q.data.len() as i64
            );
            let handles: Vec<_> = q.handles.iter().map(|v| v.raw).collect();
            let r = rt.call(
                q.channel.raw,
                q.data,
                &handles,
                q.deadline_nanos,
                q.max_reply_bytes as usize,
                q.max_reply_handles as usize,
            );
            let st = status(&r);
            let (bytes, handles) = match r {
                Ok(m) => (m.bytes, m.handles),
                Err(_) => (Vec::new(), Vec::new()),
            };
            let refs: Vec<_> = handles.iter().map(|v| h(*v)).collect();
            reply!(ChannelControlCallResponse {
                status: st,
                data: &bytes,
                handles: &refs
            })
        }
        (1, Some("ReadCall")) => {
            bexos_trace::trace_flow_step!(
                bexos_trace::CATEGORY_IPC_MESSAGES,
                "kernel:channel_read_call",
                q_channel_flow_id(ordinal)
            );
            let q = decode!(ChannelControlReadCallRequest);
            let r = rt.read_call(
                q.channel.raw,
                q.deadline_nanos,
                q.max_bytes as usize,
                q.max_handles as usize,
            );
            let st = status(&r);
            let (bytes, handles, token) = match r {
                Ok((m, token)) => (m.bytes, m.handles, token),
                Err(_) => (Vec::new(), Vec::new(), 0),
            };
            let refs: Vec<_> = handles.iter().map(|v| h(*v)).collect();
            reply!(ChannelControlReadCallResponse {
                status: st,
                data: &bytes,
                handles: &refs,
                reply_token: h(token)
            })
        }
        (1, Some("ReplyCall")) => {
            bexos_trace::trace_flow_end!(
                bexos_trace::CATEGORY_IPC_MESSAGES,
                "kernel:channel_reply_call",
                q_channel_flow_id(ordinal)
            );
            let q = decode!(ChannelControlReplyCallRequest);
            let handles: Vec<_> = q.handles.iter().map(|v| v.raw).collect();
            let r = rt.reply_call(q.reply_token.raw, q.data, &handles);
            reply!(ChannelControlReplyCallResponse { status: status(&r) })
        }
        (10, Some("CreateSocketPair")) => {
            let (a, b) = rt.create_socket_pair();
            reply!(SocketControlCreateSocketPairResponse {
                status: Status::Ok,
                local_socket: h(a),
                remote_socket: h(b)
            })
        }
        (10, Some("Read")) => {
            let q = decode!(SocketControlReadRequest);
            let r = rt.read_socket(q.socket.raw, q.max_bytes as usize);
            let st = status(&r);
            let bytes = r.unwrap_or_default();
            reply!(SocketControlReadResponse {
                status: st,
                data: &bytes
            })
        }
        (10, Some("Write")) => {
            let q = decode!(SocketControlWriteRequest);
            let r = rt.write_socket(q.socket.raw, q.data);
            reply!(SocketControlWriteResponse {
                status: status(&r),
                actual: r.unwrap_or(0) as u64
            })
        }
        (10, Some("Shutdown")) => {
            let q = decode!(SocketControlShutdownRequest);
            let r = rt.shutdown_socket(q.socket.raw, q.read, q.write);
            reply!(SocketControlShutdownResponse { status: status(&r) })
        }
        (10, Some("GetInfo")) => {
            let q = decode!(SocketControlGetInfoRequest);
            let r = rt.socket_info(q.socket.raw);
            let st = status(&r);
            let info = r.unwrap_or(bexos_kernel_core::runtime::SocketInfo {
                readable_bytes: 0,
                local_read_closed: false,
                local_write_closed: false,
                peer_read_closed: false,
                peer_write_closed: false,
            });
            reply!(SocketControlGetInfoResponse {
                status: st,
                readable_bytes: info.readable_bytes,
                local_read_closed: info.local_read_closed,
                local_write_closed: info.local_write_closed,
                peer_read_closed: info.peer_read_closed,
                peer_write_closed: info.peer_write_closed
            })
        }
        (2, Some("CreateVmo")) => {
            let q = decode!(VirtualMemoryCreateVmoRequest);
            let r = rt.create_vmo(q.size_bytes, q.flags.0);
            reply!(VirtualMemoryCreateVmoResponse {
                status: status(&r),
                vmo: h(r.unwrap_or(0))
            })
        }
        (2, Some("Map")) => {
            let q = decode!(VirtualMemoryMapRequest);
            let r = rt.map(
                None,
                q.vmo.raw,
                q.vmo_offset,
                q.size_bytes,
                q.target_vaddr,
                q.requested_rights.0,
            );
            reply!(VirtualMemoryMapResponse {
                status: status(&r),
                mapped_vaddr: r.unwrap_or(0)
            })
        }
        (2, Some("MapInVmSpace")) => {
            let q = decode!(VirtualMemoryMapInVmSpaceRequest);
            let r = rt.map(
                Some(q.vm_space.raw),
                q.vmo.raw,
                q.vmo_offset,
                q.size_bytes,
                q.target_vaddr,
                q.requested_rights.0,
            );
            reply!(VirtualMemoryMapInVmSpaceResponse {
                status: status(&r),
                mapped_vaddr: r.unwrap_or(0)
            })
        }
        (2, Some("Unmap")) => {
            let q = decode!(VirtualMemoryUnmapRequest);
            let r = rt.unmap(None, q.vaddr, q.size_bytes);
            reply!(VirtualMemoryUnmapResponse { status: status(&r) })
        }
        (2, Some("UnmapInVmSpace")) => {
            let q = decode!(VirtualMemoryUnmapInVmSpaceRequest);
            let r = rt.unmap(Some(q.vm_space.raw), q.vaddr, q.size_bytes);
            reply!(VirtualMemoryUnmapInVmSpaceResponse { status: status(&r) })
        }
        (2, Some("CreateSubVmar")) => {
            let q = decode!(VirtualMemoryCreateSubVmarRequest);
            let r = rt.create_sub_vmar(q.parent_vmar.raw, q.offset, q.size_bytes, q.flags.0);
            let st = status(&r);
            let (sub_vmar, base_address) = r.unwrap_or((0, 0));
            reply!(VirtualMemoryCreateSubVmarResponse {
                status: st,
                sub_vmar: h(sub_vmar),
                base_address
            })
        }
        (2, Some("MapVmo")) => {
            let q = decode!(VirtualMemoryMapVmoRequest);
            let r = rt.map_vmo_in_vmar(
                q.vmar.raw,
                q.vmo.raw,
                q.vmo_offset,
                q.vmar_offset,
                q.size_bytes,
                q.flags.0,
            );
            reply!(VirtualMemoryMapVmoResponse {
                status: status(&r),
                mapped_vaddr: r.unwrap_or(0)
            })
        }
        (2, Some("UnmapVmar")) => {
            let q = decode!(VirtualMemoryUnmapVmarRequest);
            let r = rt.unmap_in_vmar(q.vmar.raw, q.vaddr, q.size_bytes);
            reply!(VirtualMemoryUnmapVmarResponse { status: status(&r) })
        }
        (2, Some("DestroyVmar")) => {
            let q = decode!(VirtualMemoryDestroyVmarRequest);
            let r = rt.destroy_vmar(q.vmar.raw);
            reply!(VirtualMemoryDestroyVmarResponse { status: status(&r) })
        }
        (2, Some("CommitRange")) => {
            let q = decode!(VirtualMemoryCommitRangeRequest);
            let r = rt.commit_user_range(q.vaddr, q.size_bytes);
            reply!(VirtualMemoryCommitRangeResponse { status: status(&r) })
        }
        (3, Some("GetRuntimeStats")) => {
            let _ = decode!(TaskControlGetRuntimeStatsRequest);
            rt.scheduler
                .account_runtime(crate::arch::CurrentArch::monotonic_ns());
            let (thread_cpu_ns, process_cpu_ns) = rt
                .scheduler
                .runtime_stats(rt.current_thread as u64 + 1)
                .ok_or(FidlWireError::Transport)?;
            reply!(TaskControlGetRuntimeStatsResponse {
                status: Status::Ok,
                thread_cpu_ns,
                process_cpu_ns
            })
        }
        (3, Some("CreateThread")) => {
            let q = decode!(TaskControlCreateThreadRequest);
            let r = rt.create_thread_current(q.entry_vaddr, q.stack_top_vaddr, q.arg_handle.raw);
            reply!(TaskControlCreateThreadResponse {
                status: status(&r),
                thread_handle: h(r.unwrap_or(0))
            })
        }
        (3, Some("ExitThread")) => {
            let q = decode!(TaskControlExitThreadRequest);
            rt.exit_current_with_code(q.exit_code);
            reply!(TaskControlExitThreadResponse {})
        }
        (3, Some("FutexWait")) => {
            let q = decode!(TaskControlFutexWaitRequest);
            let r = rt.futex_wait_current_timeout(q.uaddr, q.expected_val, q.timeout_nanos);
            reply!(TaskControlFutexWaitResponse { status: status(&r) })
        }
        (3, Some("FutexWake")) => {
            let q = decode!(TaskControlFutexWakeRequest);
            let r = rt.futex_wake(q.uaddr, q.wake_count);
            reply!(TaskControlFutexWakeResponse {
                status: status(&r),
                woken_count: r.unwrap_or(0)
            })
        }
        (3, Some("WaitMany")) => {
            let q = decode!(TaskControlWaitManyRequest);
            let mut items = Vec::new();
            items
                .try_reserve_exact(q.items.len())
                .map_err(|_| FidlWireError::Transport)?;
            for index in 0..q.items.len() {
                let item = q.items.get(index)?;
                items.push((item.h.raw, item.signals.0));
            }
            let r = rt.wait_many(&items, q.deadline_nanos);
            let (satisfied_index, observed_signals) = r.unwrap_or((0, 0));
            reply!(TaskControlWaitManyResponse {
                status: status(&r),
                satisfied_index: satisfied_index as u32,
                observed_signals: Signals(observed_signals)
            })
        }
        (3, Some("SetProfile")) => {
            let q = decode!(TaskControlSetProfileRequest);
            let r = rt.set_thread_profile(q.thread.raw, q.profile.raw);
            reply!(TaskControlSetProfileResponse { status: status(&r) })
        }
        (3, Some("SetCpuAffinity")) => {
            let q = decode!(TaskControlSetCpuAffinityRequest);
            let r = rt.set_thread_cpu_affinity(q.thread.raw, q.affinity.mask);
            reply!(TaskControlSetCpuAffinityResponse { status: status(&r) })
        }
        (3, Some("YieldThread")) => {
            let q = decode!(TaskControlYieldThreadRequest);
            let r = rt.yield_current_on_cpu(
                crate::arch::CurrentArch::current_cpu_id() as u8,
                q.target_thread.raw,
            );
            reply!(TaskControlYieldThreadResponse { status: status(&r) })
        }
        (11, Some("CreateProfile")) => {
            let q = decode!(ProfileProviderCreateProfileRequest);
            let r = rt.create_scheduling_profile(from_fidl_profile(q.info));
            reply!(ProfileProviderCreateProfileResponse {
                status: status(&r),
                profile_handle: h(r.unwrap_or(0))
            })
        }
        (13, Some("GetBytes")) => {
            let q = decode!(RandomGetBytesRequest);
            let result = rt.random_bytes(q.length as usize);
            let status = status(&result);
            let bytes = result.unwrap_or_default();
            reply!(RandomGetBytesResponse {
                status,
                data: &bytes
            })
        }
        (8, Some("GetTime")) => {
            let q = decode!(ClockGetTimeRequest);
            let r = rt.get_time_ns(q.clock_type);
            reply!(ClockGetTimeResponse {
                status: status(&r),
                nanos: r.unwrap_or(0)
            })
        }
        (8, Some("GetVdsoTimePage")) => {
            let _q = decode!(ClockGetVdsoTimePageRequest);
            let r = rt.get_vdso_time_page(
                crate::arch::CurrentArch::timer_ticks(),
                crate::arch::CurrentArch::timer_frequency(),
            );
            reply!(ClockGetVdsoTimePageResponse {
                status: status(&r),
                vmo: h(r.unwrap_or(0))
            })
        }
        (4, Some("CreateProcess")) => {
            let q = decode!(SystemPrivilegedCreateProcessRequest);
            let r = rt.create_process_with_policy(
                q.name,
                q.package_id,
                q.hardware_access as u32,
                q.resource_group_id,
                q.realtime_scheduling,
            );
            let st = status(&r);
            let (a, b) = r.unwrap_or((0, 0));
            let root_vmar = if st == Status::Ok {
                rt.root_vmar_construction_handle(b).unwrap_or(0)
            } else {
                0
            };
            if st == Status::Ok {
                crate::log_line(&alloc::format!(
                    "kernel: process heap vmar ready package={} base={:#x} bytes={}",
                    q.package_id,
                    bexos_boot::USER_HEAP_VMAR_BASE,
                    bexos_boot::USER_HEAP_VMAR_SIZE
                ));
            }
            reply!(SystemPrivilegedCreateProcessResponse {
                status: st,
                process_handle: h(a),
                address_space_handle: h(b),
                root_vmar_handle: h(root_vmar)
            })
        }
        (4, Some("StartThreadInProcess")) => {
            let q = decode!(SystemPrivilegedStartThreadInProcessRequest);
            let r = rt.start_with_thread_pointer(
                q.process_handle.raw,
                q.address_space_handle.raw,
                q.entry_vaddr,
                q.stack_top_vaddr,
                q.thread_pointer_vaddr,
                q.arg_handle.raw,
            );
            reply!(SystemPrivilegedStartThreadInProcessResponse {
                status: status(&r),
                thread_handle: h(r.unwrap_or(0))
            })
        }
        (4, Some("RequestSystemPowerState")) => {
            let q = decode!(SystemPrivilegedRequestSystemPowerStateRequest);
            reply!(SystemPrivilegedRequestSystemPowerStateResponse {
                status: crate::arch::CurrentArch::request_system_power_state(q.state)
            })
        }
        (4, Some("AdjustClock")) => {
            let q = decode!(SystemPrivilegedAdjustClockRequest);
            let r = rt.adjust_clock(
                q.clock_type,
                q.offset_delta_ns,
                q.slew_rate_ppm,
                crate::arch::CurrentArch::timer_ticks(),
                crate::arch::CurrentArch::timer_frequency(),
            );
            let status = status(&r);
            reply!(SystemPrivilegedAdjustClockResponse { status })
        }
        (4, Some("TerminateProcess")) => {
            let q = decode!(SystemPrivilegedTerminateProcessRequest);
            let r = rt.terminate_process(q.process_handle.raw, q.exit_code);
            reply!(SystemPrivilegedTerminateProcessResponse { status: status(&r) })
        }
        (4, Some("GetProcessStatus")) => {
            let q = decode!(SystemPrivilegedGetProcessStatusRequest);
            let r = rt.process_status(q.process_handle.raw);
            let (exited, suspended, exit_code, process_id) =
                r.as_ref().copied().unwrap_or((false, false, 0, 0));
            reply!(SystemPrivilegedGetProcessStatusResponse {
                status: status(&r),
                exited,
                suspended,
                exit_code,
                process_id
            })
        }
        (4, Some("SuspendProcess")) => {
            let q = decode!(SystemPrivilegedSuspendProcessRequest);
            let r = rt.set_process_suspended(q.process_handle.raw, true);
            reply!(SystemPrivilegedSuspendProcessResponse { status: status(&r) })
        }
        (4, Some("ResumeProcess")) => {
            let q = decode!(SystemPrivilegedResumeProcessRequest);
            let r = rt.set_process_suspended(q.process_handle.raw, false);
            reply!(SystemPrivilegedResumeProcessResponse { status: status(&r) })
        }
        (5, Some("Close")) => {
            let q = decode!(ObjectControlCloseRequest);
            let r = rt.close(q.object.raw);
            reply!(ObjectControlCloseResponse { status: status(&r) })
        }
        (5, Some("Duplicate")) => {
            let q = decode!(ObjectControlDuplicateRequest);
            let r = rt.duplicate(q.object.raw, q.rights.0);
            reply!(ObjectControlDuplicateResponse {
                status: status(&r),
                duplicate: h(r.unwrap_or(0))
            })
        }
        (5, Some("CreatePhysicalVmo")) => {
            let q = decode!(ObjectControlCreatePhysicalVmoRequest);
            let r = rt.physical_vmo(q.base, q.size_bytes);
            reply!(ObjectControlCreatePhysicalVmoResponse {
                status: status(&r),
                vmo: h(r.unwrap_or(0))
            })
        }
        (5, Some("CreateSharedDeviceVmo")) => {
            let q = decode!(ObjectControlCreateSharedDeviceVmoRequest);
            let r = rt.shared_device_vmo(q.base, q.size_bytes);
            reply!(ObjectControlCreateSharedDeviceVmoResponse {
                status: status(&r),
                vmo: h(r.unwrap_or(0))
            })
        }
        (5, Some("IsSharedDeviceVmoIdle")) => {
            let q = decode!(ObjectControlIsSharedDeviceVmoIdleRequest);
            let r = rt.shared_device_vmo_idle(q.vmo.raw);
            reply!(ObjectControlIsSharedDeviceVmoIdleResponse {
                status: status(&r),
                idle: r.unwrap_or(false)
            })
        }
        (5, Some("Pin")) => {
            let q = decode!(ObjectControlPinRequest);
            let r = rt.pin(q.vmo.raw);
            if r == Err(Status::ErrAccessDenied) {
                let process = &rt.processes[rt.current];
                crate::log_line(&alloc::format!(
                    "kernel: pin denied package={} authority={} hardware={} quarantined={}",
                    process.package,
                    process.authority,
                    process.hardware,
                    process.quarantined
                ));
            }
            let st = status(&r);
            let (pa, token) = r.unwrap_or((0, 0));
            reply!(ObjectControlPinResponse {
                status: st,
                physical_address: pa,
                token
            })
        }
        (5, Some("Unpin")) => {
            let q = decode!(ObjectControlUnpinRequest);
            let r = rt.unpin(q.token);
            reply!(ObjectControlUnpinResponse { status: status(&r) })
        }
        (5, Some("CreateIommuDomain")) => {
            let q = decode!(ObjectControlCreateIommuDomainRequest);
            let r = rt.create_iommu_domain(q.stream_id, q.address_width);
            reply!(ObjectControlCreateIommuDomainResponse {
                status: status(&r),
                domain: h(r.unwrap_or(0))
            })
        }
        (5, Some("MapDma")) => {
            let q = decode!(ObjectControlMapDmaRequest);
            let r = rt.map_dma(q.domain.raw, q.vmo.raw, q.offset, q.length, q.permissions);
            let st = status(&r);
            let (device_address, token) = r.unwrap_or((0, 0));
            reply!(ObjectControlMapDmaResponse {
                status: st,
                device_address,
                token
            })
        }
        (5, Some("UnmapDma")) => {
            let q = decode!(ObjectControlUnmapDmaRequest);
            let r = rt.unmap_dma(q.token);
            reply!(ObjectControlUnmapDmaResponse { status: status(&r) })
        }
        (5, Some("GetInfo")) => {
            let q = decode!(ObjectControlGetInfoRequest);
            let r = rt.capability(q.object.raw, 0);
            let st = status(&r);
            let (kind, rights) = r
                .map(|cap| {
                    use bexos_kernel_core::runtime::Object;
                    let kind = match cap.object {
                        Object::Channel(..) => ObjectType::Channel,
                        Object::Socket(..) => ObjectType::Socket,
                        Object::Vmo(..) => ObjectType::Vmo,
                        Object::Process(..) => ObjectType::Process,
                        _ => ObjectType::Other,
                    };
                    (kind, cap.rights)
                })
                .unwrap_or((ObjectType::Other, 0));
            reply!(ObjectControlGetInfoResponse {
                status: st,
                kind,
                rights: Rights(rights)
            })
        }
        (5, Some("GetChannelIdentity")) => {
            let q = decode!(ObjectControlGetChannelIdentityRequest);
            let r = rt.channel_identity(q.channel.raw);
            let st = status(&r);
            let (endpoint, peer) = r.unwrap_or((0, 0));
            reply!(ObjectControlGetChannelIdentityResponse {
                status: st,
                endpoint,
                peer
            })
        }
        (5, Some("MemoryStats")) => reply!(ObjectControlMemoryStatsResponse {
            status: Status::Ok,
            free_pages: rt.backend.free_pages(),
            bootfs_pages: rt.bootfs_pages,
            reclaimed_pages: rt.reclaimed_pages,
            reused_pages: rt.backend.reused_pages()
        }),
        (6, Some("ListProcesses")) => {
            let _q = decode!(KernelDebugControlListProcessesRequest);
            let mut processes = Vec::new();
            processes
                .try_reserve_exact(rt.processes.len())
                .map_err(|_| FidlWireError::Transport)?;
            let groups =
                bexos_kernel_core::kernel_services::system::ResourceGroupTable::with_defaults();
            for (index, process) in rt.processes.iter().enumerate() {
                let group = groups.get(process.resource_group_id);
                let parent_id = rt
                    .scheduler
                    .resource_group(process.resource_group_id)
                    .and_then(|g| g.parent_id)
                    .or_else(|| group.and_then(|g| g.parent_id));
                let mut name = [0; 64];
                name[..process.name.len()].copy_from_slice(process.name.as_bytes());
                let mut package_id = [0; 96];
                package_id[..process.package.len()].copy_from_slice(process.package.as_bytes());
                processes.push(KernelProcessDebugInfo {
                    pid: index as u64 + 1,
                    main_thread_id: rt
                        .threads
                        .iter()
                        .position(|t| t.process == index)
                        .map_or(0, |i| i as u64 + 1),
                    resource_group_id: process.resource_group_id,
                    resource_group_name_len: group.map_or(0, |g| g.name_len as u32),
                    resource_group_name: group.map_or([0; 24], |g| g.name),
                    parent_resource_group_id: parent_id.unwrap_or(0),
                    name_len: process.name.len() as u32,
                    name,
                    package_id_len: process.package.len() as u32,
                    package_id,
                    state: if process.exited {
                        DebugProcessState::Exited
                    } else if process.running {
                        DebugProcessState::Running
                    } else {
                        DebugProcessState::Created
                    },
                });
            }
            reply!(KernelDebugControlListProcessesResponse {
                status: Status::Ok,
                processes: WireVector::from_slice(&processes)
            })
        }
        (6, Some("ApplyPlatformUpdate")) => {
            if !rt.has_authority(bexos_kernel_core::runtime::handover::AUTH_PLATFORM_UPDATE) {
                return reply!(KernelDebugControlApplyPlatformUpdateResponse {
                    status: Status::ErrAccessDenied,
                    message: "debugd privilege required"
                });
            }
            let q = decode!(KernelDebugControlApplyPlatformUpdateRequest);
            let mut hash = [0; 32];
            if q.artifact_hash.len() != 32 {
                return reply!(KernelDebugControlApplyPlatformUpdateResponse {
                    status: Status::ErrInvalidArgs,
                    message: "platform update hash must be 32 bytes"
                });
            }
            hash.copy_from_slice(q.artifact_hash);
            let artifact_base = match platform_artifact_base(rt, q.artifact.raw, q.artifact_len) {
                Ok(base) => base,
                Err(status) => {
                    return reply!(KernelDebugControlApplyPlatformUpdateResponse {
                        status,
                        message: "platform update artifact VMO rejected"
                    });
                }
            };
            let artifact = unsafe {
                core::slice::from_raw_parts(artifact_base as *const u8, q.artifact_len as usize)
            };
            let result = match q.kind {
                KernelUpdateKind::Microkernel => {
                    crate::transplant::stage_transplant(rt, q.generation, q.target, hash, artifact)
                }
                KernelUpdateKind::TeeImage => {
                    Err("TEE replacement requires Secure World orchestrator support")
                }
                KernelUpdateKind::Hypervisor => {
                    // This component must never enter the kernel transplant
                    // loader or masquerade as a Trusty image.
                    Err("hypervisor replacement requires the resident monitor recovery backend")
                }
            };
            match result {
                Ok(message) => reply!(KernelDebugControlApplyPlatformUpdateResponse {
                    status: Status::Ok,
                    message
                }),
                Err(message) => reply!(KernelDebugControlApplyPlatformUpdateResponse {
                    status: Status::ErrInvalidArgs,
                    message
                }),
            }
        }
        (6, Some("GetUpdateStatus")) => {
            let (raw_status, generation, message) = crate::transplant::update_status();
            let update_status = match raw_status {
                1 => KernelUpdateStatus::Idle,
                2 => KernelUpdateStatus::Staged,
                3 => KernelUpdateStatus::ApplyingTransplant,
                4 => KernelUpdateStatus::Completed,
                _ => KernelUpdateStatus::Failed,
            };
            reply!(KernelDebugControlGetUpdateStatusResponse {
                status: Status::Ok,
                update_status,
                generation,
                message
            })
        }
        (12, Some("AttachKernelProducer")) => {
            if !rt.has_authority(bexos_kernel_core::runtime::handover::AUTH_PLATFORM_UPDATE) {
                return reply!(KernelTraceControlAttachKernelProducerResponse {
                    status: Status::ErrAccessDenied
                });
            }
            let q = decode!(KernelTraceControlAttachKernelProducerRequest);
            reply!(KernelTraceControlAttachKernelProducerResponse {
                status: crate::tracing::attach(q.buffer.raw, q.mapped_len, q.producer_id, q.cpu_id)
            })
        }
        (12, Some("DetachKernelProducer")) => {
            if !rt.has_authority(bexos_kernel_core::runtime::handover::AUTH_PLATFORM_UPDATE) {
                return reply!(KernelTraceControlDetachKernelProducerResponse {
                    status: Status::ErrAccessDenied,
                    buffer: HandleRef { raw: 0 },
                    producer_id: 0,
                    mapped_len: 0
                });
            }
            match crate::tracing::detach() {
                Ok(producer) => reply!(KernelTraceControlDetachKernelProducerResponse {
                    status: Status::Ok,
                    buffer: HandleRef {
                        raw: producer.buffer
                    },
                    producer_id: producer.producer_id,
                    mapped_len: producer.mapped_len
                }),
                Err(status) => reply!(KernelTraceControlDetachKernelProducerResponse {
                    status,
                    buffer: HandleRef { raw: 0 },
                    producer_id: 0,
                    mapped_len: 0
                }),
            }
        }
        (9, Some("Call")) => {
            if !rt.has_authority(bexos_kernel_core::runtime::handover::AUTH_SECURE_MONITOR) {
                crate::log_line(&alloc::format!(
                    "kernel: secure-monitor call denied package={} authority={}",
                    rt.processes[rt.current].package,
                    rt.processes[rt.current].authority
                ));
                return reply!(SecureMonitorCallResponse {
                    status: Status::ErrAccessDenied,
                    x0: 0,
                    x1: 0,
                    x2: 0,
                    x3: 0,
                    x4: 0,
                    x5: 0,
                    x6: 0,
                    x7: 0,
                });
            }
            let q = decode!(SecureMonitorCallRequest);
            if matches!(q.x0, 0x3200_001e | 0x3200_0020 | 0x3c00_0003)
                && TRUSTY_SMC_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 24
            {
                crate::log_line(&alloc::format!(
                    "kernel: Trusty SMC cpu={} fid={:#x}",
                    crate::arch::CurrentArch::current_cpu_id(),
                    q.x0
                ));
            }
            let result = {
                let invocation = bexos_kernel_core::kernel_services::tee::SmcInvocation {
                    regs: [q.x0, q.x1, q.x2, q.x3, q.x4, q.x5, q.x6, q.x7],
                };
                if invocation.regs[0] == 0 {
                    bexos_kernel_core::kernel_services::tee::SmcResult {
                        status:
                            bexos_kernel_core::kernel_services::KernelServiceStatus::InvalidArgs,
                        regs: [0; 8],
                    }
                } else {
                    bexos_kernel_core::kernel_services::tee::SmcResult {
                        status: bexos_kernel_core::kernel_services::KernelServiceStatus::Ok,
                        regs: {
                            #[cfg(target_arch = "x86_64")]
                            {
                                match crate::secure_memory::authorize(rt, invocation.regs) {
                                    Ok(registers) => {
                                        crate::arch::CurrentArch::invoke_smc(registers)
                                    }
                                    Err(status) => status.registers(0),
                                }
                            }
                            #[cfg(not(target_arch = "x86_64"))]
                            {
                                crate::arch::CurrentArch::invoke_smc(invocation.regs)
                            }
                        },
                    }
                }
            };
            reply!(SecureMonitorCallResponse {
                status: bexos_kernel_core::kernel_services::fidl::to_fidl_status(result.status),
                x0: result.regs[0],
                x1: result.regs[1],
                x2: result.regs[2],
                x3: result.regs[3],
                x4: result.regs[4],
                x5: result.regs[5],
                x6: result.regs[6],
                x7: result.regs[7],
            })
        }
        _ => Err(FidlWireError::UnknownOrdinal(ordinal)),
    }
}

fn platform_artifact_base(rt: &mut Rt, handle: u64, len: u64) -> Result<u64, Status> {
    if len == 0 || len > 16 * 1024 * 1024 {
        return Err(Status::ErrInvalidArgs);
    }
    let id = rt.vmo_for(handle, 2)?;
    let vmo = rt.vmos[id].as_ref().ok_or(Status::ErrInvalidHandle)?;
    if len > vmo.size || vmo.device {
        return Err(Status::ErrInvalidArgs);
    }
    let base = rt.materialize_vmo_for_kernel_slice(id)?;
    // CPU0 cannot schedule a VMO writer inside this syscall, and secondary CPUs
    // never run EL0. Read the physical backing without duplicating it on the
    // smaller kernel heap; staging copies the validated ELF into reserved RAM.
    Ok(base)
}
