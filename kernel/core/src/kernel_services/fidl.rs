use alloc::vec::Vec;
use kernel_fidl::{
    ChannelControlCreateChannelRequest, ChannelControlCreateChannelResponse,
    ChannelControlPublicServer, ChannelControlReadMessageRequest,
    ChannelControlReadMessageResponse, ChannelControlWriteMessageRequest,
    ChannelControlWriteMessageResponse, ClockGetTimeRequest, ClockGetTimeResponse,
    ClockGetVdsoTimePageResponse, ClockPublicServer, FidlWireError, HandleRef, InlineVectorStruct1,
    KernelDebugControlApplyPlatformUpdateRequest, KernelDebugControlApplyPlatformUpdateResponse,
    KernelDebugControlGetUpdateStatusRequest, KernelDebugControlGetUpdateStatusResponse,
    KernelDebugControlListProcessesRequest, KernelDebugControlListProcessesResponse,
    KernelDebugControlPublicServer, KernelUpdateStatus, Rights, Status, SystemPowerState,
    SystemPrivilegedAcknowledgeInterruptRequest, SystemPrivilegedAcknowledgeInterruptResponse,
    SystemPrivilegedAdjustClockRequest, SystemPrivilegedAdjustClockResponse,
    SystemPrivilegedBexosSystemPrivilegedServer, SystemPrivilegedBindInterruptRequest,
    SystemPrivilegedBindInterruptResponse, SystemPrivilegedCheckpointSystemStateRequest,
    SystemPrivilegedCheckpointSystemStateResponse, SystemPrivilegedCreateProcessRequest,
    SystemPrivilegedCreateProcessResponse, SystemPrivilegedCreateResourceGroupRequest,
    SystemPrivilegedCreateResourceGroupResponse, SystemPrivilegedGetResourceGroupRequest,
    SystemPrivilegedGetResourceGroupResponse, SystemPrivilegedMaskInterruptRequest,
    SystemPrivilegedMaskInterruptResponse, SystemPrivilegedRequestSystemPowerStateRequest,
    SystemPrivilegedRequestSystemPowerStateResponse, SystemPrivilegedSetResourceGroupLimitsRequest,
    SystemPrivilegedSetResourceGroupLimitsResponse, SystemPrivilegedSetTimeServer,
    SystemPrivilegedStartThreadInProcessRequest, SystemPrivilegedStartThreadInProcessResponse,
    SystemPrivilegedTerminateProcessRequest, SystemPrivilegedTerminateProcessResponse,
    TaskControlCreateThreadRequest, TaskControlCreateThreadResponse, TaskControlExitThreadRequest,
    TaskControlFutexWaitRequest, TaskControlFutexWaitResponse, TaskControlFutexWakeRequest,
    TaskControlFutexWakeResponse, TaskControlPublicServer, TaskControlSetCpuAffinityRequest,
    TaskControlSetCpuAffinityResponse, TaskControlSetProfileRequest, TaskControlSetProfileResponse,
    TaskControlWaitManyRequest, TaskControlWaitManyResponse, TaskControlYieldThreadRequest,
    TaskControlYieldThreadResponse, VirtualMemoryCloneVmoRequest, VirtualMemoryCloneVmoResponse,
    VirtualMemoryCommitRangeRequest, VirtualMemoryCommitRangeResponse,
    VirtualMemoryCreateSubVmarRequest, VirtualMemoryCreateSubVmarResponse,
    VirtualMemoryCreateVmoRequest, VirtualMemoryCreateVmoResponse, VirtualMemoryDestroyVmarRequest,
    VirtualMemoryDestroyVmarResponse, VirtualMemoryMapInVmSpaceRequest,
    VirtualMemoryMapInVmSpaceResponse, VirtualMemoryMapRequest, VirtualMemoryMapResponse,
    VirtualMemoryMapVmoRequest, VirtualMemoryMapVmoResponse, VirtualMemoryPublicServer,
    VirtualMemoryUnmapInVmSpaceRequest, VirtualMemoryUnmapInVmSpaceResponse,
    VirtualMemoryUnmapRequest, VirtualMemoryUnmapResponse, VirtualMemoryUnmapVmarRequest,
    VirtualMemoryUnmapVmarResponse, VmarFlags, VmoFlags, WireVector,
};
use kernel_fidl::{
    SystemPrivilegedGetProcessStatusRequest, SystemPrivilegedGetProcessStatusResponse,
    SystemPrivilegedResumeProcessRequest, SystemPrivilegedResumeProcessResponse,
    SystemPrivilegedSuspendProcessRequest, SystemPrivilegedSuspendProcessResponse,
};

use super::ipc::ChannelPolicy;
use super::power;
use super::{ControlPlane, Handle, HardwareAccess, KernelServiceStatus, WaitManyItem};
use crate::sched::{DeadlineProfile, FairProfile, SchedulingProfile};

impl ChannelControlPublicServer for ControlPlane {
    fn create_channel<'a>(
        &mut self,
        _request: ChannelControlCreateChannelRequest,
    ) -> Result<ChannelControlCreateChannelResponse, FidlWireError> {
        Ok(match self.create_channel() {
            Ok((local_endpoint, remote_endpoint)) => ChannelControlCreateChannelResponse {
                status: Status::Ok,
                local_endpoint: to_ref(local_endpoint),
                remote_endpoint: to_ref(remote_endpoint),
            },
            Err(status) => ChannelControlCreateChannelResponse {
                status: to_fidl_status(status),
                local_endpoint: HandleRef { raw: 0 },
                remote_endpoint: HandleRef { raw: 0 },
            },
        })
    }

    fn read_message<'a>(
        &mut self,
        request: ChannelControlReadMessageRequest,
    ) -> Result<ChannelControlReadMessageResponse<'a>, FidlWireError> {
        let result = self.read_message(
            from_ref(request.channel),
            request.max_bytes as usize,
            request.max_handles as usize,
        );
        self.scratch.handle_refs.clear();
        if self
            .scratch
            .handle_refs
            .try_reserve_exact(result.handles_len)
            .is_err()
        {
            return Ok(ChannelControlReadMessageResponse {
                status: Status::ErrNoMemory,
                data: &[],
                handles: &[],
            });
        }
        self.scratch.handle_refs.extend(
            self.scratch.handles[..result.handles_len]
                .iter()
                .copied()
                .map(to_ref),
        );

        let data = &self.scratch.bytes[..result.bytes_len];
        let handles = &self.scratch.handle_refs[..result.handles_len];
        Ok(ChannelControlReadMessageResponse {
            status: to_fidl_status(result.status),
            data: unsafe { core::mem::transmute::<&[u8], &'a [u8]>(data) },
            handles: unsafe { core::mem::transmute::<&[HandleRef], &'a [HandleRef]>(handles) },
        })
    }

    fn write_message<'a>(
        &mut self,
        request: ChannelControlWriteMessageRequest<'a>,
    ) -> Result<ChannelControlWriteMessageResponse, FidlWireError> {
        let mut handles = alloc::vec::Vec::new();
        if handles.try_reserve_exact(request.handles.len()).is_err() {
            return Ok(ChannelControlWriteMessageResponse {
                status: Status::ErrNoMemory,
            });
        }
        handles.extend(request.handles.iter().copied().map(from_ref));
        Ok(ChannelControlWriteMessageResponse {
            status: to_fidl_status(self.write_message(
                from_ref(request.channel),
                request.data,
                &handles,
            )),
        })
    }

    fn set_policy<'a>(
        &mut self,
        request: kernel_fidl::ChannelControlSetPolicyRequest,
    ) -> Result<kernel_fidl::ChannelControlSetPolicyResponse, FidlWireError> {
        Ok(kernel_fidl::ChannelControlSetPolicyResponse {
            status: to_fidl_status(self.set_channel_policy(
                from_ref(request.channel),
                ChannelPolicy {
                    enable_priority_inheritance: request.enable_priority_inheritance,
                    enable_timeslice_donation: request.enable_timeslice_donation,
                },
            )),
        })
    }

    fn call<'a>(
        &mut self,
        request: kernel_fidl::ChannelControlCallRequest<'a>,
    ) -> Result<kernel_fidl::ChannelControlCallResponse<'a>, FidlWireError> {
        let handles = request
            .handles
            .iter()
            .copied()
            .map(from_ref)
            .collect::<Vec<_>>();
        let result = self.call(
            from_ref(request.channel),
            request.data,
            &handles,
            request.deadline_nanos,
        );
        self.scratch.handle_refs.clear();
        self.scratch
            .handle_refs
            .extend(self.scratch.handles.iter().copied().map(to_ref));
        Ok(kernel_fidl::ChannelControlCallResponse {
            status: to_fidl_status(result.status),
            data: unsafe {
                core::mem::transmute::<&[u8], &'a [u8]>(&self.scratch.bytes[..result.bytes_len])
            },
            handles: unsafe {
                core::mem::transmute::<&[HandleRef], &'a [HandleRef]>(
                    &self.scratch.handle_refs[..result.handles_len],
                )
            },
        })
    }

    fn read_call<'a>(
        &mut self,
        request: kernel_fidl::ChannelControlReadCallRequest,
    ) -> Result<kernel_fidl::ChannelControlReadCallResponse<'a>, FidlWireError> {
        let (result, token) = self.read_call(
            from_ref(request.channel),
            request.max_bytes as usize,
            request.max_handles as usize,
            request.deadline_nanos,
        );
        self.scratch.handle_refs.clear();
        self.scratch
            .handle_refs
            .extend(self.scratch.handles.iter().copied().map(to_ref));
        Ok(kernel_fidl::ChannelControlReadCallResponse {
            status: to_fidl_status(result.status),
            data: unsafe {
                core::mem::transmute::<&[u8], &'a [u8]>(&self.scratch.bytes[..result.bytes_len])
            },
            handles: unsafe {
                core::mem::transmute::<&[HandleRef], &'a [HandleRef]>(
                    &self.scratch.handle_refs[..result.handles_len],
                )
            },
            reply_token: to_ref(token),
        })
    }

    fn reply_call<'a>(
        &mut self,
        request: kernel_fidl::ChannelControlReplyCallRequest<'a>,
    ) -> Result<kernel_fidl::ChannelControlReplyCallResponse, FidlWireError> {
        Ok(kernel_fidl::ChannelControlReplyCallResponse {
            status: to_fidl_status(
                self.reply_call(
                    from_ref(request.reply_token),
                    request.data,
                    &request
                        .handles
                        .iter()
                        .copied()
                        .map(from_ref)
                        .collect::<Vec<_>>(),
                ),
            ),
        })
    }
}

impl kernel_fidl::ProfileProviderPublicServer for ControlPlane {
    fn create_profile<'a>(
        &mut self,
        request: kernel_fidl::ProfileProviderCreateProfileRequest,
    ) -> Result<kernel_fidl::ProfileProviderCreateProfileResponse, FidlWireError> {
        Ok(
            match self.create_scheduling_profile(from_fidl_profile(request.info)) {
                Ok(profile_handle) => kernel_fidl::ProfileProviderCreateProfileResponse {
                    status: Status::Ok,
                    profile_handle: to_ref(profile_handle),
                },
                Err(status) => kernel_fidl::ProfileProviderCreateProfileResponse {
                    status: to_fidl_status(status),
                    profile_handle: HandleRef { raw: 0 },
                },
            },
        )
    }
}

impl ClockPublicServer for ControlPlane {
    fn get_time<'a>(
        &mut self,
        request: ClockGetTimeRequest,
    ) -> Result<ClockGetTimeResponse, FidlWireError> {
        let result = self.clock_get_time(request.clock_type);
        Ok(ClockGetTimeResponse {
            status: to_fidl_status(result.status),
            nanos: result.nanos,
        })
    }

    fn get_vdso_time_page<'a>(
        &mut self,
        _request: kernel_fidl::ClockGetVdsoTimePageRequest,
    ) -> Result<ClockGetVdsoTimePageResponse, FidlWireError> {
        Ok(match self.clock_get_vdso_time_page() {
            Ok(vmo) => ClockGetVdsoTimePageResponse {
                status: Status::Ok,
                vmo: to_ref(vmo),
            },
            Err(status) => ClockGetVdsoTimePageResponse {
                status: to_fidl_status(status),
                vmo: HandleRef { raw: 0 },
            },
        })
    }
}

impl VirtualMemoryPublicServer for ControlPlane {
    fn create_vmo<'a>(
        &mut self,
        request: VirtualMemoryCreateVmoRequest,
    ) -> Result<VirtualMemoryCreateVmoResponse, FidlWireError> {
        let VmoFlags(flags) = request.flags;
        Ok(match self.create_vmo(request.size_bytes, flags) {
            Ok(vmo) => VirtualMemoryCreateVmoResponse {
                status: Status::Ok,
                vmo: to_ref(vmo),
            },
            Err(status) => VirtualMemoryCreateVmoResponse {
                status: to_fidl_status(status),
                vmo: HandleRef { raw: 0 },
            },
        })
    }

    fn map<'a>(
        &mut self,
        request: VirtualMemoryMapRequest,
    ) -> Result<VirtualMemoryMapResponse, FidlWireError> {
        let Rights(rights) = request.requested_rights;
        let result = self.map_vmo(
            from_ref(request.vmo),
            request.vmo_offset,
            request.size_bytes,
            request.target_vaddr,
            rights,
        );
        Ok(VirtualMemoryMapResponse {
            status: to_fidl_status(result.status),
            mapped_vaddr: result.mapped_vaddr,
        })
    }

    fn unmap<'a>(
        &mut self,
        request: VirtualMemoryUnmapRequest,
    ) -> Result<VirtualMemoryUnmapResponse, FidlWireError> {
        Ok(VirtualMemoryUnmapResponse {
            status: to_fidl_status(self.unmap(request.vaddr, request.size_bytes)),
        })
    }

    fn map_in_vm_space<'a>(
        &mut self,
        request: VirtualMemoryMapInVmSpaceRequest,
    ) -> Result<VirtualMemoryMapInVmSpaceResponse, FidlWireError> {
        let Rights(rights) = request.requested_rights;
        let result = self.map_vmo_in_vm_space(
            from_ref(request.vm_space),
            from_ref(request.vmo),
            request.vmo_offset,
            request.size_bytes,
            request.target_vaddr,
            rights,
        );
        Ok(VirtualMemoryMapInVmSpaceResponse {
            status: to_fidl_status(result.status),
            mapped_vaddr: result.mapped_vaddr,
        })
    }

    fn unmap_in_vm_space<'a>(
        &mut self,
        request: VirtualMemoryUnmapInVmSpaceRequest,
    ) -> Result<VirtualMemoryUnmapInVmSpaceResponse, FidlWireError> {
        Ok(VirtualMemoryUnmapInVmSpaceResponse {
            status: to_fidl_status(self.unmap_in_vm_space(
                from_ref(request.vm_space),
                request.vaddr,
                request.size_bytes,
            )),
        })
    }

    fn clone_vmo<'a>(
        &mut self,
        request: VirtualMemoryCloneVmoRequest,
    ) -> Result<VirtualMemoryCloneVmoResponse, FidlWireError> {
        Ok(
            match self.clone_vmo(
                from_ref(request.parent_vmo),
                request.offset,
                request.size_bytes,
            ) {
                Ok(cloned_vmo) => VirtualMemoryCloneVmoResponse {
                    status: Status::Ok,
                    cloned_vmo: to_ref(cloned_vmo),
                },
                Err(status) => VirtualMemoryCloneVmoResponse {
                    status: to_fidl_status(status),
                    cloned_vmo: HandleRef { raw: 0 },
                },
            },
        )
    }

    fn create_sub_vmar<'a>(
        &mut self,
        request: VirtualMemoryCreateSubVmarRequest,
    ) -> Result<VirtualMemoryCreateSubVmarResponse, FidlWireError> {
        let VmarFlags(flags) = request.flags;
        Ok(
            match self.create_sub_vmar(
                from_ref(request.parent_vmar),
                request.offset,
                request.size_bytes,
                flags,
            ) {
                Ok((sub_vmar, base_address)) => VirtualMemoryCreateSubVmarResponse {
                    status: Status::Ok,
                    sub_vmar: to_ref(sub_vmar),
                    base_address,
                },
                Err(status) => VirtualMemoryCreateSubVmarResponse {
                    status: to_fidl_status(status),
                    sub_vmar: HandleRef { raw: 0 },
                    base_address: 0,
                },
            },
        )
    }

    fn map_vmo<'a>(
        &mut self,
        request: VirtualMemoryMapVmoRequest,
    ) -> Result<VirtualMemoryMapVmoResponse, FidlWireError> {
        let VmarFlags(flags) = request.flags;
        let result = ControlPlane::map_vmo_in_vmar(
            self,
            from_ref(request.vmar),
            from_ref(request.vmo),
            request.vmo_offset,
            request.vmar_offset,
            request.size_bytes,
            flags,
        );
        Ok(VirtualMemoryMapVmoResponse {
            status: to_fidl_status(result.status),
            mapped_vaddr: result.mapped_vaddr,
        })
    }

    fn unmap_vmar<'a>(
        &mut self,
        request: VirtualMemoryUnmapVmarRequest,
    ) -> Result<VirtualMemoryUnmapVmarResponse, FidlWireError> {
        Ok(VirtualMemoryUnmapVmarResponse {
            status: to_fidl_status(ControlPlane::unmap_in_vmar(
                self,
                from_ref(request.vmar),
                request.vaddr,
                request.size_bytes,
            )),
        })
    }

    fn destroy_vmar<'a>(
        &mut self,
        request: VirtualMemoryDestroyVmarRequest,
    ) -> Result<VirtualMemoryDestroyVmarResponse, FidlWireError> {
        Ok(VirtualMemoryDestroyVmarResponse {
            status: to_fidl_status(ControlPlane::destroy_vmar(self, from_ref(request.vmar))),
        })
    }

    fn commit_range<'a>(
        &mut self,
        request: VirtualMemoryCommitRangeRequest,
    ) -> Result<VirtualMemoryCommitRangeResponse, FidlWireError> {
        Ok(VirtualMemoryCommitRangeResponse {
            status: to_fidl_status(ControlPlane::commit_range(
                self,
                request.vaddr,
                request.size_bytes,
            )),
        })
    }
}

impl TaskControlPublicServer for ControlPlane {
    fn get_runtime_stats<'a>(
        &mut self,
        _: kernel_fidl::TaskControlGetRuntimeStatsRequest,
    ) -> Result<kernel_fidl::TaskControlGetRuntimeStatsResponse, FidlWireError> {
        let stats = self
            .scheduler
            .current()
            .and_then(|t| self.scheduler.runtime_stats(t.id));
        let (thread_cpu_ns, process_cpu_ns) = stats.unwrap_or_default();
        Ok(kernel_fidl::TaskControlGetRuntimeStatsResponse {
            status: if stats.is_some() {
                Status::Ok
            } else {
                Status::ErrInvalidArgs
            },
            thread_cpu_ns,
            process_cpu_ns,
        })
    }

    fn create_thread<'a>(
        &mut self,
        request: TaskControlCreateThreadRequest,
    ) -> Result<TaskControlCreateThreadResponse, FidlWireError> {
        let arg_handle = (request.arg_handle.raw != 0).then_some(from_ref(request.arg_handle));
        Ok(
            match self.create_thread(request.entry_vaddr, request.stack_top_vaddr, arg_handle) {
                Ok(thread_handle) => TaskControlCreateThreadResponse {
                    status: Status::Ok,
                    thread_handle: to_ref(thread_handle),
                },
                Err(status) => TaskControlCreateThreadResponse {
                    status: to_fidl_status(status),
                    thread_handle: HandleRef { raw: 0 },
                },
            },
        )
    }

    fn exit_thread<'a>(
        &mut self,
        request: TaskControlExitThreadRequest,
    ) -> Result<(), FidlWireError> {
        let _ = ControlPlane::exit_thread(self, request.exit_code);
        Ok(())
    }

    fn futex_wait<'a>(
        &mut self,
        request: TaskControlFutexWaitRequest,
    ) -> Result<TaskControlFutexWaitResponse, FidlWireError> {
        Ok(TaskControlFutexWaitResponse {
            status: to_fidl_status(self.futex_wait(
                request.uaddr,
                request.expected_val,
                request.timeout_nanos,
                (request.owner_thread.raw != 0).then_some(from_ref(request.owner_thread)),
            )),
        })
    }

    fn futex_wake<'a>(
        &mut self,
        request: TaskControlFutexWakeRequest,
    ) -> Result<TaskControlFutexWakeResponse, FidlWireError> {
        let result = self.futex_wake(request.uaddr, request.wake_count);
        Ok(TaskControlFutexWakeResponse {
            status: to_fidl_status(result.status),
            woken_count: result.woken_count,
        })
    }

    fn wait_many<'a>(
        &mut self,
        request: TaskControlWaitManyRequest<'a>,
    ) -> Result<TaskControlWaitManyResponse, FidlWireError> {
        let mut items = alloc::vec::Vec::new();
        if items.try_reserve_exact(request.items.len()).is_err() {
            return Ok(TaskControlWaitManyResponse {
                status: Status::ErrNoMemory,
                satisfied_index: u32::MAX,
                observed_signals: kernel_fidl::Signals(0),
            });
        }
        for index in 0..request.items.len() {
            items.push(from_wait_item(request.items.get(index)?));
        }
        let result = ControlPlane::wait_many(self, &items, request.deadline_nanos);
        Ok(TaskControlWaitManyResponse {
            status: to_fidl_status(result.status),
            satisfied_index: result.satisfied_index,
            observed_signals: kernel_fidl::Signals(result.observed_signals),
        })
    }

    fn set_profile<'a>(
        &mut self,
        request: TaskControlSetProfileRequest,
    ) -> Result<TaskControlSetProfileResponse, FidlWireError> {
        Ok(TaskControlSetProfileResponse {
            status: to_fidl_status(
                self.set_thread_profile(from_ref(request.thread), from_ref(request.profile)),
            ),
        })
    }

    fn set_cpu_affinity<'a>(
        &mut self,
        request: TaskControlSetCpuAffinityRequest,
    ) -> Result<TaskControlSetCpuAffinityResponse, FidlWireError> {
        Ok(TaskControlSetCpuAffinityResponse {
            status: to_fidl_status(
                self.set_thread_cpu_affinity(from_ref(request.thread), request.affinity.mask),
            ),
        })
    }

    fn yield_thread<'a>(
        &mut self,
        request: TaskControlYieldThreadRequest,
    ) -> Result<TaskControlYieldThreadResponse, FidlWireError> {
        Ok(TaskControlYieldThreadResponse {
            status: to_fidl_status(self.yield_thread(
                (request.target_thread.raw != 0).then_some(from_ref(request.target_thread)),
            )),
        })
    }
}

impl SystemPrivilegedBexosSystemPrivilegedServer for ControlPlane {
    fn create_process<'a>(
        &mut self,
        request: SystemPrivilegedCreateProcessRequest<'a>,
    ) -> Result<SystemPrivilegedCreateProcessResponse, FidlWireError> {
        Ok(
            match self.create_process_with_realtime_and_root(
                request.name,
                request.resource_group_id,
                request.package_id,
                from_fidl_hardware_access(request.hardware_access),
                request.realtime_scheduling,
            ) {
                Ok((process_handle, address_space_handle, root_vmar_handle)) => {
                    SystemPrivilegedCreateProcessResponse {
                        status: Status::Ok,
                        process_handle: to_ref(process_handle),
                        address_space_handle: to_ref(address_space_handle),
                        root_vmar_handle: to_ref(root_vmar_handle),
                    }
                }
                Err(status) => SystemPrivilegedCreateProcessResponse {
                    status: to_fidl_status(status),
                    process_handle: HandleRef { raw: 0 },
                    address_space_handle: HandleRef { raw: 0 },
                    root_vmar_handle: HandleRef { raw: 0 },
                },
            },
        )
    }

    fn open_resource_group<'a>(
        &mut self,
        request: kernel_fidl::SystemPrivilegedOpenResourceGroupRequest,
    ) -> Result<kernel_fidl::SystemPrivilegedOpenResourceGroupResponse, FidlWireError> {
        let result = self
            .resource_groups
            .get(request.resource_group_id)
            .ok_or(KernelServiceStatus::InvalidHandle)
            .and_then(|group| {
                self.handles.insert(
                    u64::from(group.id),
                    super::handle::ObjectKind::ResourceGroup,
                    super::RIGHT_ADMIN
                        | super::RIGHT_READ
                        | super::RIGHT_SET_POLICY
                        | super::RIGHT_TRANSFER,
                    0,
                    None,
                )
            });
        Ok(match result {
            Ok(group_handle) => kernel_fidl::SystemPrivilegedOpenResourceGroupResponse {
                status: Status::Ok,
                group_handle: to_ref(group_handle),
            },
            Err(status) => kernel_fidl::SystemPrivilegedOpenResourceGroupResponse {
                status: to_fidl_status(status),
                group_handle: HandleRef { raw: 0 },
            },
        })
    }

    fn create_resource_group_v2<'a>(
        &mut self,
        request: kernel_fidl::SystemPrivilegedCreateResourceGroupV2Request<'a>,
    ) -> Result<kernel_fidl::SystemPrivilegedCreateResourceGroupV2Response, FidlWireError> {
        let parent = (request.parent_group.raw != 0).then_some(from_ref(request.parent_group));
        Ok(
            match self.create_resource_group_v2(
                request.name,
                parent,
                request.cpu_shares,
                request.max_cpu_utilization_permille,
                request.allow_realtime,
                request.memory_low_watermark_bytes,
                request.memory_high_watermark_bytes,
                request.max_render_budget_percent,
                request.max_vram_bytes,
            ) {
                Ok((resource_group_id, group_handle)) => {
                    kernel_fidl::SystemPrivilegedCreateResourceGroupV2Response {
                        status: Status::Ok,
                        resource_group_id,
                        group_handle: to_ref(group_handle),
                    }
                }
                Err(status) => kernel_fidl::SystemPrivilegedCreateResourceGroupV2Response {
                    status: to_fidl_status(status),
                    resource_group_id: 0,
                    group_handle: HandleRef { raw: 0 },
                },
            },
        )
    }

    fn set_resource_group_limits_v2<'a>(
        &mut self,
        request: kernel_fidl::SystemPrivilegedSetResourceGroupLimitsV2Request,
    ) -> Result<kernel_fidl::SystemPrivilegedSetResourceGroupLimitsV2Response, FidlWireError> {
        Ok(
            kernel_fidl::SystemPrivilegedSetResourceGroupLimitsV2Response {
                status: to_fidl_status(
                    self.set_resource_group_limits_v2(
                        from_ref(request.group_handle),
                        request.cpu_shares,
                        request.max_cpu_utilization_permille,
                        request.allow_realtime,
                        request.memory_low_watermark_bytes,
                        request.memory_high_watermark_bytes,
                        request.max_render_budget_percent,
                        request.max_vram_bytes,
                    )
                    .map(|_| KernelServiceStatus::Ok)
                    .unwrap_or_else(|status| status),
                ),
            },
        )
    }

    fn get_resource_group_v2<'a>(
        &mut self,
        request: kernel_fidl::SystemPrivilegedGetResourceGroupV2Request,
    ) -> Result<kernel_fidl::SystemPrivilegedGetResourceGroupV2Response<'a>, FidlWireError> {
        let result = ControlPlane::get_resource_group(self, from_ref(request.group_handle));
        Ok(match result {
            Ok(group) => {
                self.scratch.bytes.clear();
                self.scratch
                    .bytes
                    .extend_from_slice(group.name_str().unwrap_or("").as_bytes());
                let name = core::str::from_utf8(&self.scratch.bytes).unwrap_or("");
                kernel_fidl::SystemPrivilegedGetResourceGroupV2Response {
                    status: Status::Ok,
                    resource_group_id: group.id,
                    parent_resource_group_id: group.parent_id.unwrap_or(0),
                    name: unsafe { core::mem::transmute::<&str, &'a str>(name) },
                    cpu_shares: group.cpu_shares,
                    max_cpu_utilization_permille: group.max_cpu_utilization_permille,
                    allow_realtime: group.allow_realtime,
                    memory_low_watermark_bytes: group.memory_low_watermark_bytes,
                    memory_high_watermark_bytes: group.memory_high_watermark_bytes,
                    memory_used_bytes: group.memory_used_bytes,
                    memory_pressure: group.memory_low_watermark_bytes != 0
                        && group.memory_used_bytes >= group.memory_low_watermark_bytes,
                    max_render_budget_percent: group.max_render_budget_percent,
                    max_vram_bytes: group.max_vram_bytes,
                    reserved_render_budget_percent: group.reserved_render_budget_percent,
                    reserved_vram_bytes: group.reserved_vram_bytes,
                }
            }
            Err(status) => kernel_fidl::SystemPrivilegedGetResourceGroupV2Response {
                status: to_fidl_status(status),
                resource_group_id: 0,
                parent_resource_group_id: 0,
                name: "",
                cpu_shares: 0,
                max_cpu_utilization_permille: 0,
                allow_realtime: false,
                memory_low_watermark_bytes: 0,
                memory_high_watermark_bytes: 0,
                memory_used_bytes: 0,
                memory_pressure: false,
                max_render_budget_percent: 0,
                max_vram_bytes: 0,
                reserved_render_budget_percent: 0,
                reserved_vram_bytes: 0,
            },
        })
    }

    fn reserve_gpu_resources<'a>(
        &mut self,
        request: kernel_fidl::SystemPrivilegedReserveGpuResourcesRequest,
    ) -> Result<kernel_fidl::SystemPrivilegedReserveGpuResourcesResponse, FidlWireError> {
        Ok(
            match self.reserve_gpu_resources(
                from_ref(request.group_handle),
                request.render_budget_percent,
                request.vram_bytes,
            ) {
                Ok(reservation_handle) => {
                    kernel_fidl::SystemPrivilegedReserveGpuResourcesResponse {
                        status: Status::Ok,
                        reservation_handle: to_ref(reservation_handle),
                    }
                }
                Err(status) => kernel_fidl::SystemPrivilegedReserveGpuResourcesResponse {
                    status: to_fidl_status(status),
                    reservation_handle: HandleRef { raw: 0 },
                },
            },
        )
    }

    fn bind_interrupt<'a>(
        &mut self,
        request: SystemPrivilegedBindInterruptRequest,
    ) -> Result<SystemPrivilegedBindInterruptResponse, FidlWireError> {
        Ok(
            match self.bind_interrupt(request.irq_number, request.flags) {
                Ok(irq_handle) => SystemPrivilegedBindInterruptResponse {
                    status: Status::Ok,
                    irq_handle: to_ref(irq_handle),
                },
                Err(status) => SystemPrivilegedBindInterruptResponse {
                    status: to_fidl_status(status),
                    irq_handle: HandleRef { raw: 0 },
                },
            },
        )
    }

    fn acknowledge_interrupt<'a>(
        &mut self,
        request: SystemPrivilegedAcknowledgeInterruptRequest,
    ) -> Result<SystemPrivilegedAcknowledgeInterruptResponse, FidlWireError> {
        Ok(SystemPrivilegedAcknowledgeInterruptResponse {
            status: match self.acknowledge_interrupt(from_ref(request.irq_handle)) {
                Ok(()) => Status::Ok,
                Err(status) => to_fidl_status(status),
            },
        })
    }

    fn mask_interrupt<'a>(
        &mut self,
        request: SystemPrivilegedMaskInterruptRequest,
    ) -> Result<SystemPrivilegedMaskInterruptResponse, FidlWireError> {
        Ok(SystemPrivilegedMaskInterruptResponse {
            status: match self.mask_interrupt(from_ref(request.irq_handle), request.masked) {
                Ok(()) => Status::Ok,
                Err(status) => to_fidl_status(status),
            },
        })
    }

    fn create_resource_group<'a>(
        &mut self,
        request: SystemPrivilegedCreateResourceGroupRequest<'a>,
    ) -> Result<SystemPrivilegedCreateResourceGroupResponse, FidlWireError> {
        Ok(
            match ControlPlane::create_resource_group(
                self,
                request.name,
                request.cpu_shares,
                request.memory_limit_pages,
            ) {
                Ok((resource_group_id, group_handle)) => {
                    SystemPrivilegedCreateResourceGroupResponse {
                        status: Status::Ok,
                        resource_group_id,
                        group_handle: to_ref(group_handle),
                    }
                }
                Err(status) => SystemPrivilegedCreateResourceGroupResponse {
                    status: to_fidl_status(status),
                    resource_group_id: 0,
                    group_handle: HandleRef { raw: 0 },
                },
            },
        )
    }

    fn set_resource_group_limits<'a>(
        &mut self,
        request: SystemPrivilegedSetResourceGroupLimitsRequest,
    ) -> Result<SystemPrivilegedSetResourceGroupLimitsResponse, FidlWireError> {
        Ok(SystemPrivilegedSetResourceGroupLimitsResponse {
            status: to_fidl_status(
                ControlPlane::set_resource_group_limits(
                    self,
                    from_ref(request.group_handle),
                    request.cpu_shares,
                    request.memory_limit_pages,
                )
                .map(|_| KernelServiceStatus::Ok)
                .unwrap_or_else(|status| status),
            ),
        })
    }

    fn get_resource_group<'a>(
        &mut self,
        request: SystemPrivilegedGetResourceGroupRequest,
    ) -> Result<SystemPrivilegedGetResourceGroupResponse<'a>, FidlWireError> {
        Ok(
            match ControlPlane::get_resource_group(self, from_ref(request.group_handle)) {
                Ok(group) => {
                    self.scratch.bytes.clear();
                    let name = group.name_str().unwrap_or("").as_bytes();
                    self.scratch
                        .bytes
                        .try_reserve_exact(name.len())
                        .map_err(|_| FidlWireError::BufferTooSmall)?;
                    self.scratch.bytes.extend_from_slice(name);
                    let name = core::str::from_utf8(&self.scratch.bytes).unwrap_or("");
                    SystemPrivilegedGetResourceGroupResponse {
                        status: Status::Ok,
                        resource_group_id: group.id,
                        name: unsafe { core::mem::transmute::<&str, &'a str>(name) },
                        cpu_shares: group.cpu_shares,
                        memory_limit_pages: group.memory_limit_pages,
                    }
                }
                Err(status) => SystemPrivilegedGetResourceGroupResponse {
                    status: to_fidl_status(status),
                    resource_group_id: 0,
                    name: "",
                    cpu_shares: 0,
                    memory_limit_pages: 0,
                },
            },
        )
    }

    fn start_thread_in_process<'a>(
        &mut self,
        request: SystemPrivilegedStartThreadInProcessRequest,
    ) -> Result<SystemPrivilegedStartThreadInProcessResponse, FidlWireError> {
        let arg_handle = (request.arg_handle.raw != 0).then_some(from_ref(request.arg_handle));
        Ok(
            match ControlPlane::start_thread_in_process_with_thread_pointer(
                self,
                from_ref(request.process_handle),
                from_ref(request.address_space_handle),
                request.entry_vaddr,
                request.stack_top_vaddr,
                request.thread_pointer_vaddr,
                arg_handle,
            ) {
                Ok(thread_handle) => SystemPrivilegedStartThreadInProcessResponse {
                    status: Status::Ok,
                    thread_handle: to_ref(thread_handle),
                },
                Err(status) => SystemPrivilegedStartThreadInProcessResponse {
                    status: to_fidl_status(status),
                    thread_handle: HandleRef { raw: 0 },
                },
            },
        )
    }

    fn checkpoint_system_state<'a>(
        &mut self,
        request: SystemPrivilegedCheckpointSystemStateRequest,
    ) -> Result<SystemPrivilegedCheckpointSystemStateResponse, FidlWireError> {
        let result = ControlPlane::checkpoint_system_state(self, from_ref(request.target_vmo));
        Ok(SystemPrivilegedCheckpointSystemStateResponse {
            status: to_fidl_status(result.status),
            serialized_bytes: result.serialized_bytes,
        })
    }

    fn request_system_power_state<'a>(
        &mut self,
        request: SystemPrivilegedRequestSystemPowerStateRequest,
    ) -> Result<SystemPrivilegedRequestSystemPowerStateResponse, FidlWireError> {
        Ok(SystemPrivilegedRequestSystemPowerStateResponse {
            status: to_fidl_status(ControlPlane::request_system_power_state(
                self,
                from_fidl_power_state(request.state),
            )),
        })
    }

    fn get_process_status(
        &mut self,
        request: SystemPrivilegedGetProcessStatusRequest,
    ) -> Result<SystemPrivilegedGetProcessStatusResponse, FidlWireError> {
        Ok(
            match self.process_for_handle(from_ref(request.process_handle)) {
                Ok(process) => SystemPrivilegedGetProcessStatusResponse {
                    status: Status::Ok,
                    process_id: process.id,
                    exited: process.state == super::system::ProcessState::Exited,
                    suspended: self
                        .scheduler
                        .task(process.main_thread_id)
                        .is_some_and(|t| t.state == crate::sched::TaskState::Stopped),
                    exit_code: self
                        .threads
                        .get(process.main_thread_id)
                        .map_or(0, |t| t.exit_code),
                },
                Err(e) => SystemPrivilegedGetProcessStatusResponse {
                    status: to_fidl_status(e),
                    process_id: 0,
                    exited: false,
                    suspended: false,
                    exit_code: 0,
                },
            },
        )
    }
    fn suspend_process(
        &mut self,
        request: SystemPrivilegedSuspendProcessRequest,
    ) -> Result<SystemPrivilegedSuspendProcessResponse, FidlWireError> {
        let status = match self.process_for_handle(from_ref(request.process_handle)) {
            Ok(process) if process.state != super::system::ProcessState::Exited => {
                self.scheduler.stop_process(process.id);
                Status::Ok
            }
            Ok(_) => Status::ErrInvalidArgs,
            Err(e) => to_fidl_status(e),
        };
        Ok(SystemPrivilegedSuspendProcessResponse { status })
    }
    fn resume_process(
        &mut self,
        request: SystemPrivilegedResumeProcessRequest,
    ) -> Result<SystemPrivilegedResumeProcessResponse, FidlWireError> {
        let status = match self.process_for_handle(from_ref(request.process_handle)) {
            Ok(process) if process.state != super::system::ProcessState::Exited => {
                self.scheduler.resume_stopped_process(process.id);
                Status::Ok
            }
            Ok(_) => Status::ErrInvalidArgs,
            Err(e) => to_fidl_status(e),
        };
        Ok(SystemPrivilegedResumeProcessResponse { status })
    }
    fn terminate_process<'a>(
        &mut self,
        request: SystemPrivilegedTerminateProcessRequest,
    ) -> Result<SystemPrivilegedTerminateProcessResponse, FidlWireError> {
        Ok(SystemPrivilegedTerminateProcessResponse {
            status: to_fidl_status(ControlPlane::terminate_process(
                self,
                from_ref(request.process_handle),
                request.exit_code,
            )),
        })
    }
}

impl SystemPrivilegedSetTimeServer for ControlPlane {
    fn adjust_clock<'a>(
        &mut self,
        request: SystemPrivilegedAdjustClockRequest,
    ) -> Result<SystemPrivilegedAdjustClockResponse, FidlWireError> {
        Ok(SystemPrivilegedAdjustClockResponse {
            status: to_fidl_status(ControlPlane::clock_adjust(
                self,
                request.clock_type,
                request.offset_delta_ns,
                request.slew_rate_ppm,
            )),
        })
    }
}

impl KernelDebugControlPublicServer for ControlPlane {
    fn list_processes<'a>(
        &mut self,
        _request: KernelDebugControlListProcessesRequest,
    ) -> Result<KernelDebugControlListProcessesResponse<'a>, FidlWireError> {
        let result = self.list_process_debug_info();
        Ok(match result {
            Ok(processes) => KernelDebugControlListProcessesResponse {
                status: Status::Ok,
                processes: WireVector::from_slice(unsafe {
                    core::mem::transmute::<_, &'a [_]>(processes)
                }),
            },
            Err(status) => KernelDebugControlListProcessesResponse {
                status: to_fidl_status(status),
                processes: WireVector::from_slice(&[]),
            },
        })
    }

    fn apply_platform_update<'a>(
        &mut self,
        request: KernelDebugControlApplyPlatformUpdateRequest<'a>,
    ) -> Result<KernelDebugControlApplyPlatformUpdateResponse<'a>, FidlWireError> {
        if request.artifact_len == 0
            || request.artifact_hash.len() != 32
            || request.target.is_empty()
            || request.artifact.raw == 0
        {
            return Ok(KernelDebugControlApplyPlatformUpdateResponse {
                status: Status::ErrInvalidArgs,
                message: "invalid platform update",
            });
        }
        Ok(KernelDebugControlApplyPlatformUpdateResponse {
            status: Status::Ok,
            message: "host control-plane platform update accepted",
        })
    }

    fn get_update_status<'a>(
        &mut self,
        _request: KernelDebugControlGetUpdateStatusRequest,
    ) -> Result<KernelDebugControlGetUpdateStatusResponse<'a>, FidlWireError> {
        Ok(KernelDebugControlGetUpdateStatusResponse {
            status: Status::Ok,
            update_status: KernelUpdateStatus::Idle,
            generation: 0,
            message: "idle",
        })
    }
}

const fn from_fidl_hardware_access(access: kernel_fidl::HardwareAccess) -> HardwareAccess {
    match access {
        kernel_fidl::HardwareAccess::None => HardwareAccess::None,
        kernel_fidl::HardwareAccess::Isolated => HardwareAccess::Isolated,
        kernel_fidl::HardwareAccess::Direct => HardwareAccess::Direct,
    }
}

const fn from_fidl_power_state(state: SystemPowerState) -> power::SystemPowerState {
    match state {
        SystemPowerState::Active => power::SystemPowerState::Active,
        SystemPowerState::SuspendToRam => power::SystemPowerState::SuspendToRam,
        SystemPowerState::SuspendToDisk => power::SystemPowerState::SuspendToDisk,
        SystemPowerState::Reboot => power::SystemPowerState::Reboot,
        SystemPowerState::Poweroff => power::SystemPowerState::Poweroff,
    }
}

pub const fn to_fidl_status(status: KernelServiceStatus) -> Status {
    match status {
        KernelServiceStatus::Ok => Status::Ok,
        KernelServiceStatus::InvalidHandle => Status::ErrInvalidHandle,
        KernelServiceStatus::AccessDenied => Status::ErrAccessDenied,
        KernelServiceStatus::NoMemory => Status::ErrNoMemory,
        KernelServiceStatus::BufferTooSmall => Status::ErrBufferTooSmall,
        KernelServiceStatus::PeerClosed => Status::ErrPeerClosed,
        KernelServiceStatus::TimedOut => Status::ErrTimedOut,
        KernelServiceStatus::AlreadyExists => Status::ErrAlreadyExists,
        KernelServiceStatus::InvalidArgs => Status::ErrInvalidArgs,
        KernelServiceStatus::ResourceExhausted => Status::ErrResourceExhausted,
    }
}

const fn to_ref(handle: Handle) -> HandleRef {
    HandleRef { raw: handle.raw }
}

const fn from_fidl_profile(info: kernel_fidl::SchedulingProfileInfo) -> SchedulingProfile {
    match info {
        kernel_fidl::SchedulingProfileInfo::Deadline(deadline) => {
            SchedulingProfile::Deadline(DeadlineProfile {
                capacity_ns: deadline.capacity_ns,
                deadline_ns: deadline.deadline_ns,
                period_ns: deadline.period_ns,
            })
        }
        kernel_fidl::SchedulingProfileInfo::Fair(fair) => SchedulingProfile::Fair(FairProfile {
            priority: fair.priority,
            weight: fair.weight,
        }),
    }
}

const fn from_ref(handle: HandleRef) -> Handle {
    Handle { raw: handle.raw }
}

const fn from_wait_item(item: InlineVectorStruct1) -> WaitManyItem {
    WaitManyItem {
        handle: from_ref(item.h),
        signals: item.signals.0,
    }
}
