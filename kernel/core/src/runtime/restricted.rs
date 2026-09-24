use super::*;
#[cfg(not(bexos_arch_x86_64))]
use bexos_restricted_abi::Aarch64StateV1;
#[cfg(bexos_arch_x86_64)]
use bexos_restricted_abi::X86_64StateV1;
use bexos_restricted_abi::{Architecture, Header, Reason, STATE_VMO_SIZE};

#[cfg(bexos_arch_x86_64)]
const USER_RFLAGS: u64 = (1 << 0)
    | (1 << 2)
    | (1 << 4)
    | (1 << 6)
    | (1 << 7)
    | (1 << 8)
    | (1 << 9)
    | (1 << 10)
    | (1 << 11)
    | (1 << 16)
    | (1 << 18)
    | (1 << 21);

fn bytes_of<T>(value: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts((value as *const T).cast(), core::mem::size_of::<T>()) }
}

fn read_value<T: Copy>(bytes: &[u8]) -> T {
    assert_eq!(bytes.len(), core::mem::size_of::<T>());
    unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast()) }
}

impl<B: Backend> Runtime<B> {
    pub fn restricted_bind_state(&mut self, options: u32, state_handle: u64) -> Result<()> {
        if options != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        if self.threads[self.current_thread].restricted.is_some() {
            return Err(Status::ErrAlreadyExists);
        }
        let id = self.vmo_for(state_handle, READ | WRITE)?;
        let vmo = self.vmos[id].as_ref().ok_or(Status::ErrInvalidHandle)?;
        if vmo.size != STATE_VMO_SIZE || vmo.device || vmo.bootfs {
            return Err(Status::ErrInvalidArgs);
        }
        self.vmos[id].as_mut().unwrap().refs += 1;
        self.threads[self.current_thread].restricted = Some(RestrictedBinding {
            state_vmo: id,
            host_context: Context::zero(),
            guest_context: Context::zero(),
            host_readonly_thread_pointer: 0,
            guest_readonly_thread_pointer: 0,
            vector_entry: 0,
            vector_context: 0,
            active: false,
            pending_kick: false,
            transition_pending: false,
        });
        self.changed(VMO, id);
        self.changed(THREAD, self.current_thread);
        Ok(())
    }

    pub fn restricted_unbind_state(&mut self, options: u32) -> Result<()> {
        if options != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        let binding = self.threads[self.current_thread]
            .restricted
            .ok_or(Status::ErrInvalidArgs)?;
        if binding.active {
            return Err(Status::ErrAlreadyExists);
        }
        self.threads[self.current_thread].restricted = None;
        self.release_vmo(binding.state_vmo);
        self.changed(THREAD, self.current_thread);
        Ok(())
    }

    pub fn restricted_is_active(&self) -> bool {
        self.threads
            .get(self.current_thread)
            .and_then(|thread| thread.restricted)
            .is_some_and(|binding| binding.active)
    }

    pub fn restricted_is_active_on_cpu(&self, cpu_id: u8) -> bool {
        self.scheduler
            .current_on_cpu(cpu_id)
            .and_then(|task| self.threads.get(task.id as usize - 1))
            .and_then(|thread| thread.restricted)
            .is_some_and(|binding| binding.active)
    }

    pub fn restricted_enter(
        &mut self,
        options: u32,
        vector_entry: u64,
        vector_context: u64,
        host_context: Context,
        host_readonly_thread_pointer: u64,
    ) -> Result<Context> {
        if options != 0
            || !self.valid_range(self.current, vector_entry, 1, EXECUTE)
            || host_context.stack_pointer < 16
            || !self.valid_range(self.current, host_context.stack_pointer - 16, 16, WRITE)
        {
            return Err(Status::ErrInvalidArgs);
        }
        let binding = self.threads[self.current_thread]
            .restricted
            .ok_or(Status::ErrInvalidArgs)?;
        if binding.active {
            return Err(Status::ErrAlreadyExists);
        }
        let mut guest = binding.guest_context;
        let guest_readonly_thread_pointer =
            self.read_restricted_state(binding.state_vmo, &mut guest)?;
        let pending_kick = binding.pending_kick;
        let binding = self.threads[self.current_thread]
            .restricted
            .as_mut()
            .unwrap();
        binding.host_context = host_context;
        binding.guest_context = guest;
        binding.host_readonly_thread_pointer = host_readonly_thread_pointer;
        binding.guest_readonly_thread_pointer = guest_readonly_thread_pointer;
        binding.vector_entry = vector_entry;
        binding.vector_context = vector_context;
        binding.pending_kick = false;
        binding.active = !pending_kick;
        binding.transition_pending = true;
        self.changed(THREAD, self.current_thread);
        if pending_kick {
            self.finish_restricted_exit(Reason::Kick, 0, 0, guest)
        } else {
            self.threads[self.current_thread].context = guest;
            Ok(guest)
        }
    }

    pub fn restricted_exit_current(
        &mut self,
        reason: Reason,
        exception_code: u64,
        fault_address: u64,
        guest: Context,
    ) -> Result<Context> {
        if !self.restricted_is_active() {
            return Err(Status::ErrInvalidArgs);
        }
        self.finish_restricted_exit(reason, exception_code, fault_address, guest)
    }

    fn finish_restricted_exit(
        &mut self,
        reason: Reason,
        exception_code: u64,
        fault_address: u64,
        guest: Context,
    ) -> Result<Context> {
        let binding = self.threads[self.current_thread]
            .restricted
            .ok_or(Status::ErrInvalidArgs)?;
        self.write_restricted_state(
            binding.state_vmo,
            &guest,
            binding.guest_readonly_thread_pointer,
            reason,
            exception_code,
            fault_address,
        )?;
        let mut host = binding.host_context;
        host.instruction_pointer = binding.vector_entry;
        host.set_initial_argument(binding.vector_context);
        host.syscall_words_mut()[1] = reason as u32 as u64;
        #[cfg(bexos_arch_x86_64)]
        {
            host.stack_pointer = host.stack_pointer.saturating_sub(8);
            self.copy_to_user(host.stack_pointer, &0u64.to_le_bytes())?;
        }
        let binding = self.threads[self.current_thread]
            .restricted
            .as_mut()
            .unwrap();
        binding.guest_context = guest;
        binding.active = false;
        binding.pending_kick = false;
        self.threads[self.current_thread].context = host;
        self.changed(THREAD, self.current_thread);
        Ok(host)
    }

    pub fn restricted_kick(&mut self, options: u32, thread_handle: u64) -> Result<u64> {
        if options != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        let Object::Thread(thread_id) = self.capability(thread_handle, ADMIN)?.object else {
            return Err(Status::ErrInvalidHandle);
        };
        let thread = self
            .threads
            .get_mut(thread_id)
            .ok_or(Status::ErrInvalidHandle)?;
        if thread.exited || thread.restricted.is_none() {
            return Err(Status::ErrInvalidArgs);
        }
        thread.restricted.as_mut().unwrap().pending_kick = true;
        self.changed(THREAD, thread_id);
        Ok(self
            .scheduler
            .task(thread_id as u64 + 1)
            .and_then(|task| task.assigned_cpu)
            .map_or(0, |cpu| 1u64 << cpu))
    }

    pub fn restricted_deliver_pending_kick(&mut self, frame: &mut Context) -> bool {
        let pending = self.threads[self.current_thread]
            .restricted
            .is_some_and(|binding| binding.active && binding.pending_kick);
        if !pending {
            return false;
        }
        match self.finish_restricted_exit(Reason::Kick, 0, 0, *frame) {
            Ok(host) => {
                *frame = host;
                true
            }
            Err(_) => false,
        }
    }

    pub fn restricted_take_transition(&mut self) -> Option<Context> {
        let binding = self
            .threads
            .get_mut(self.current_thread)?
            .restricted
            .as_mut()?;
        if !binding.transition_pending {
            return None;
        }
        binding.transition_pending = false;
        Some(self.threads[self.current_thread].context)
    }

    pub fn restricted_capture_readonly_thread_pointer(&mut self, value: u64) {
        if let Some(binding) = self.threads[self.current_thread].restricted.as_mut()
            && binding.active
        {
            binding.guest_readonly_thread_pointer = value;
            self.changed(THREAD, self.current_thread);
        }
    }

    pub fn restricted_current_readonly_thread_pointer(&self) -> Option<u64> {
        self.threads
            .get(self.current_thread)
            .and_then(|thread| thread.restricted)
            .map(|binding| {
                if binding.active {
                    binding.guest_readonly_thread_pointer
                } else {
                    binding.host_readonly_thread_pointer
                }
            })
    }

    fn read_restricted_state(&self, vmo: usize, context: &mut Context) -> Result<u64> {
        #[cfg(bexos_arch_x86_64)]
        {
            let mut bytes = [0; core::mem::size_of::<X86_64StateV1>()];
            self.read_vmo_object(vmo, 0, &mut bytes)?;
            let state: X86_64StateV1 = read_value(&bytes);
            if !state
                .header
                .valid_for(Architecture::X86_64, core::mem::size_of::<X86_64StateV1>())
                || state.rip >= bexos_boot::USER_END
                || state.rsp >= bexos_boot::USER_END
                || state.rsp < 16
                || state.fs_base >= bexos_boot::USER_END
                || state.gs_base >= bexos_boot::USER_END
                || !self.valid_range(self.current, state.rip, 1, EXECUTE)
                || !self.valid_range(self.current, state.rsp - 16, 16, WRITE)
            {
                return Err(Status::ErrInvalidArgs);
            }
            let r = &mut context.regs;
            r[0] = state.rdi;
            r[1] = state.rsi;
            r[2] = state.rdx;
            r[3] = state.r10;
            r[4] = state.r8;
            r[5] = state.r9;
            r[6] = state.r12;
            r[7] = state.r13;
            r[8] = state.r14;
            r[9] = state.r15;
            r[10] = state.rax;
            r[11] = state.rcx;
            r[12] = state.rbx;
            r[13] = state.rbp;
            r[14] = state.r11;
            r[15] = state.gs_base;
            r[30] = 0x23;
            context.instruction_pointer = state.rip;
            context.stack_pointer = state.rsp;
            context.processor_state = (state.rflags & USER_RFLAGS) | 0x202;
            context.thread_pointer = state.fs_base;
            return Ok(0);
        }
        #[cfg(not(bexos_arch_x86_64))]
        {
            let mut bytes = [0; core::mem::size_of::<Aarch64StateV1>()];
            self.read_vmo_object(vmo, 0, &mut bytes)?;
            let state: Aarch64StateV1 = read_value(&bytes);
            if !state.header.valid_for(
                Architecture::Aarch64,
                core::mem::size_of::<Aarch64StateV1>(),
            ) || state.pc >= bexos_boot::USER_END
                || state.sp >= bexos_boot::USER_END
                || state.sp < 16
                || state.tpidr_el0 >= bexos_boot::USER_END
                || state.tpidrro_el0 >= bexos_boot::USER_END
                || !self.valid_range(self.current, state.pc, 4, EXECUTE)
                || !self.valid_range(self.current, state.sp - 16, 16, WRITE)
            {
                return Err(Status::ErrInvalidArgs);
            }
            context.regs.copy_from_slice(&state.x);
            context.stack_pointer = state.sp;
            context.instruction_pointer = state.pc;
            context.processor_state = state.pstate & 0xf000_0000;
            context.thread_pointer = state.tpidr_el0;
            return Ok(state.tpidrro_el0);
        }
    }

    fn write_restricted_state(
        &mut self,
        vmo: usize,
        context: &Context,
        _readonly_thread_pointer: u64,
        reason: Reason,
        exception_code: u64,
        fault_address: u64,
    ) -> Result<()> {
        #[cfg(bexos_arch_x86_64)]
        {
            let r = &context.regs;
            let mut state = X86_64StateV1 {
                header: Header::new(Architecture::X86_64, core::mem::size_of::<X86_64StateV1>()),
                rdi: r[0],
                rsi: r[1],
                rdx: r[2],
                rcx: r[11],
                r8: r[4],
                r9: r[5],
                rax: r[10],
                rbx: r[12],
                rbp: r[13],
                r10: r[3],
                r11: r[14],
                r12: r[6],
                r13: r[7],
                r14: r[8],
                r15: r[9],
                rip: context.instruction_pointer,
                rsp: context.stack_pointer,
                rflags: context.processor_state,
                fs_base: context.thread_pointer,
                gs_base: r[15],
            };
            state.header.reason = reason as u32;
            state.header.exception_code = exception_code;
            state.header.fault_address = fault_address;
            self.write_vmo_object(vmo, 0, bytes_of(&state))
        }
        #[cfg(not(bexos_arch_x86_64))]
        {
            let mut state = Aarch64StateV1 {
                header: Header::new(
                    Architecture::Aarch64,
                    core::mem::size_of::<Aarch64StateV1>(),
                ),
                x: context.regs,
                sp: context.stack_pointer,
                pc: context.instruction_pointer,
                pstate: context.processor_state,
                tpidr_el0: context.thread_pointer,
                tpidrro_el0: _readonly_thread_pointer,
            };
            state.header.reason = reason as u32;
            state.header.exception_code = exception_code;
            state.header.fault_address = fault_address;
            self.write_vmo_object(vmo, 0, bytes_of(&state))
        }
    }
}
