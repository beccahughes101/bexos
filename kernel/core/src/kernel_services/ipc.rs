use crate::ipc::Endpoint;

use kernel_fidl::{HandleRef, KernelProcessDebugInfo};

use super::handle::ObjectKind;
use super::{
    ControlPlane, Handle, KernelServiceStatus, RIGHT_READ, RIGHT_SET_POLICY, RIGHT_SIGNAL,
    RIGHT_TRANSFER, RIGHT_WRITE, ReadMessageResult, SIGNAL_READABLE, SIGNAL_WRITABLE,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelPolicy {
    pub enable_priority_inheritance: bool,
    pub enable_timeslice_donation: bool,
}

impl ChannelPolicy {
    pub const fn disabled() -> Self {
        Self {
            enable_priority_inheritance: false,
            enable_timeslice_donation: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferredMessage {
    pub bytes: Vec<u8>,
    pub handles: Vec<Handle>,
}

impl TransferredMessage {
    pub const fn empty() -> Self {
        Self {
            bytes: Vec::new(),
            handles: Vec::new(),
        }
    }

    pub fn new(bytes: &[u8], handles: &[Handle]) -> Result<Self, KernelServiceStatus> {
        let mut message = Self::empty();
        message
            .bytes
            .try_reserve_exact(bytes.len())
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        message
            .handles
            .try_reserve_exact(handles.len())
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        message.bytes.extend_from_slice(bytes);
        message.handles.extend_from_slice(handles);
        Ok(message)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MessageScratch {
    pub bytes: Vec<u8>,
    pub handles: Vec<Handle>,
    pub handle_refs: Vec<HandleRef>,
    pub process_debug: Vec<KernelProcessDebugInfo>,
}

impl MessageScratch {
    pub const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            handles: Vec::new(),
            handle_refs: Vec::new(),
            process_debug: Vec::new(),
        }
    }
}

impl Default for MessageScratch {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelObject {
    pub id: u64,
    pub peer_a_open: bool,
    pub peer_b_open: bool,
    pub policy: ChannelPolicy,
    pub to_a: MessageQueue,
    pub to_b: MessageQueue,
    pub calls_to_a: CallQueue,
    pub calls_to_b: CallQueue,
}

impl ChannelObject {
    pub const fn empty() -> Self {
        Self {
            id: 0,
            peer_a_open: false,
            peer_b_open: false,
            policy: ChannelPolicy::disabled(),
            to_a: MessageQueue::new(),
            to_b: MessageQueue::new(),
            calls_to_a: CallQueue::new(),
            calls_to_b: CallQueue::new(),
        }
    }

    pub const fn new(id: u64) -> Self {
        Self {
            id,
            peer_a_open: true,
            peer_b_open: true,
            policy: ChannelPolicy::disabled(),
            to_a: MessageQueue::new(),
            to_b: MessageQueue::new(),
            calls_to_a: CallQueue::new(),
            calls_to_b: CallQueue::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallRecord {
    pub id: u64,
    pub caller: Endpoint,
    pub caller_thread_id: u64,
    pub request: TransferredMessage,
    pub reply: Option<TransferredMessage>,
    pub serving: bool,
    pub server_thread_id: Option<u64>,
    pub donated_priority: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallQueue {
    entries: VecDeque<CallRecord>,
    next_id: u64,
}

impl CallQueue {
    pub const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            next_id: 1,
        }
    }
    fn push(
        &mut self,
        caller: Endpoint,
        caller_thread_id: u64,
        request: TransferredMessage,
    ) -> Result<(), KernelServiceStatus> {
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.entries.push_back(CallRecord {
            id,
            caller,
            caller_thread_id,
            request,
            reply: None,
            serving: false,
            server_thread_id: None,
            donated_priority: 0,
        });
        Ok(())
    }
    fn take_for_server(
        &mut self,
        server_thread_id: u64,
        donated_priority: u8,
    ) -> Option<CallRecord> {
        let record = self
            .entries
            .iter_mut()
            .find(|entry| !entry.serving && entry.reply.is_none())?;
        record.serving = true;
        record.server_thread_id = Some(server_thread_id);
        record.donated_priority = donated_priority;
        Some(record.clone())
    }
    fn complete(&mut self, id: u64, reply: TransferredMessage) -> Option<u64> {
        if let Some(record) = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && entry.serving)
        {
            let server_thread_id = record.server_thread_id;
            record.reply = Some(reply);
            return server_thread_id;
        }
        None
    }
    fn take_completed_for(&mut self, caller_thread_id: u64) -> Option<TransferredMessage> {
        let index = self.entries.iter().position(|entry| {
            entry.caller_thread_id == caller_thread_id && entry.reply.is_some()
        })?;
        self.entries.remove(index)?.reply
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageQueue {
    entries: VecDeque<TransferredMessage>,
}

impl MessageQueue {
    pub const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    pub fn push(&mut self, message: TransferredMessage) -> Result<(), KernelServiceStatus> {
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        self.entries.push_back(message);
        Ok(())
    }

    pub fn pop(&mut self) -> Result<TransferredMessage, KernelServiceStatus> {
        self.entries
            .pop_front()
            .ok_or(KernelServiceStatus::TimedOut)
    }

    pub fn front(&self) -> Option<&TransferredMessage> {
        self.entries.front()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChannelTable {
    entries: Vec<ChannelObject>,
    next_channel_id: u64,
}

impl ChannelTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_channel_id: 1,
        }
    }

    pub fn create(&mut self) -> Result<u64, KernelServiceStatus> {
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let id = self.next_channel_id;
        self.next_channel_id = self.next_channel_id.saturating_add(1);
        self.entries.push(ChannelObject::new(id));
        Ok(id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut ChannelObject> {
        self.entries.iter_mut().find(|channel| channel.id == id)
    }

    pub fn get(&self, id: u64) -> Option<ChannelObject> {
        self.entries
            .iter()
            .find(|channel| channel.id == id)
            .cloned()
    }
}

impl Default for ChannelTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlPlane {
    pub fn call(
        &mut self,
        channel: Handle,
        bytes: &[u8],
        handles: &[Handle],
        deadline_nanos: i64,
    ) -> ReadMessageResult {
        let Some(record) = self.handles.get(channel.raw) else {
            return read_status(KernelServiceStatus::InvalidHandle);
        };
        if record.kind != ObjectKind::Channel || !record.has_rights(RIGHT_READ | RIGHT_WRITE) {
            return read_status(KernelServiceStatus::AccessDenied);
        }
        let Some(endpoint) = record.endpoint else {
            return read_status(KernelServiceStatus::InvalidHandle);
        };
        let message = match TransferredMessage::new(bytes, handles) {
            Ok(message) => message,
            Err(status) => return read_status(status),
        };
        let caller_thread_id = self.threads.current_thread_id();
        {
            let Some(object) = self.channels.get_mut(record.object_id) else {
                return read_status(KernelServiceStatus::InvalidHandle);
            };
            let queue = match endpoint {
                Endpoint::A => &mut object.calls_to_b,
                Endpoint::B => &mut object.calls_to_a,
            };
            if let Some(reply) = queue.take_completed_for(caller_thread_id) {
                return self.deliver_message(reply, usize::MAX, usize::MAX);
            }
        }
        if deadline_expired(self.clock.monotonic_nanos(), deadline_nanos) {
            return read_status(KernelServiceStatus::TimedOut);
        }
        let Some(object) = self.channels.get_mut(record.object_id) else {
            return read_status(KernelServiceStatus::InvalidHandle);
        };
        let queue = match endpoint {
            Endpoint::A => &mut object.calls_to_b,
            Endpoint::B => &mut object.calls_to_a,
        };
        match queue.push(endpoint, caller_thread_id, message) {
            Ok(()) => read_status(KernelServiceStatus::TimedOut),
            Err(status) => read_status(status),
        }
    }

    pub fn read_call(
        &mut self,
        channel: Handle,
        max_bytes: usize,
        max_handles: usize,
        deadline_nanos: i64,
    ) -> (ReadMessageResult, Handle) {
        let Some(record) = self.handles.get(channel.raw) else {
            return (
                read_status(KernelServiceStatus::InvalidHandle),
                Handle { raw: 0 },
            );
        };
        if record.kind != ObjectKind::Channel || !record.has_rights(RIGHT_READ) {
            return (
                read_status(KernelServiceStatus::AccessDenied),
                Handle { raw: 0 },
            );
        }
        let Some(endpoint) = record.endpoint else {
            return (
                read_status(KernelServiceStatus::InvalidHandle),
                Handle { raw: 0 },
            );
        };
        let server_thread_id = self.threads.current_thread_id();
        let (call, policy) = {
            let Some(object) = self.channels.get_mut(record.object_id) else {
                return (
                    read_status(KernelServiceStatus::InvalidHandle),
                    Handle { raw: 0 },
                );
            };
            let queue = match endpoint {
                Endpoint::A => &mut object.calls_to_a,
                Endpoint::B => &mut object.calls_to_b,
            };
            let donated_priority = if object.policy.enable_priority_inheritance
                || object.policy.enable_timeslice_donation
            {
                self.threads
                    .get(server_thread_id)
                    .map(|server| server.effective_priority)
                    .unwrap_or(0)
            } else {
                0
            };
            let Some(call) = queue.take_for_server(server_thread_id, donated_priority) else {
                if deadline_expired(self.clock.monotonic_nanos(), deadline_nanos) {
                    return (
                        read_status(KernelServiceStatus::TimedOut),
                        Handle { raw: 0 },
                    );
                }
                return (
                    read_status(KernelServiceStatus::TimedOut),
                    Handle { raw: 0 },
                );
            };
            (call, object.policy)
        };
        if call.caller_thread_id != 0
            && server_thread_id != 0
            && call.caller_thread_id == server_thread_id
        {
            return (
                read_status(KernelServiceStatus::InvalidArgs),
                Handle { raw: 0 },
            );
        }
        if server_thread_id != 0
            && (policy.enable_priority_inheritance || policy.enable_timeslice_donation)
        {
            if let Some(caller) = self.threads.get(call.caller_thread_id) {
                let donated_priority = caller.effective_priority;
                let _ = self
                    .scheduler
                    .donate_priority(server_thread_id, donated_priority);
                if let Some(server) = self.threads.get_mut(server_thread_id) {
                    if donated_priority > server.effective_priority {
                        server.effective_priority = donated_priority;
                        server.priority = donated_priority;
                    }
                }
            }
        }
        let result = self.deliver_message(call.request, max_bytes, max_handles);
        let token = self
            .handles
            .insert(call.id, ObjectKind::ReplyToken, RIGHT_WRITE, 0, None)
            .unwrap_or(Handle { raw: 0 });
        (result, token)
    }

    pub fn reply_call(
        &mut self,
        token: Handle,
        bytes: &[u8],
        handles: &[Handle],
    ) -> KernelServiceStatus {
        let Some(record) = self.handles.remove(token.raw) else {
            return KernelServiceStatus::InvalidHandle;
        };
        if record.kind != ObjectKind::ReplyToken || !record.has_rights(RIGHT_WRITE) {
            return KernelServiceStatus::AccessDenied;
        }
        let reply = match TransferredMessage::new(bytes, handles) {
            Ok(reply) => reply,
            Err(status) => return status,
        };
        let mut restored_server = None;
        for object in &mut self.channels.entries {
            if let Some(server_thread_id) =
                object.calls_to_a.complete(record.object_id, reply.clone())
            {
                restored_server = Some(server_thread_id);
                break;
            }
            if let Some(server_thread_id) =
                object.calls_to_b.complete(record.object_id, reply.clone())
            {
                restored_server = Some(server_thread_id);
                break;
            }
        }
        if let Some(server_thread_id) = restored_server {
            self.restore_inherited_priority(server_thread_id);
            return KernelServiceStatus::Ok;
        }
        KernelServiceStatus::InvalidHandle
    }
    pub fn create_channel(&mut self) -> Result<(Handle, Handle), KernelServiceStatus> {
        let channel_id = self.channels.create()?;
        let rights = RIGHT_READ | RIGHT_WRITE | RIGHT_TRANSFER | RIGHT_SIGNAL | RIGHT_SET_POLICY;
        let local = self.handles.insert(
            channel_id,
            ObjectKind::Channel,
            rights,
            0,
            Some(Endpoint::A),
        )?;
        let remote = self.handles.insert(
            channel_id,
            ObjectKind::Channel,
            rights,
            0,
            Some(Endpoint::B),
        )?;
        Ok((local, remote))
    }

    pub fn set_channel_policy(
        &mut self,
        channel: Handle,
        policy: ChannelPolicy,
    ) -> KernelServiceStatus {
        let Some(record) = self.handles.get(channel.raw) else {
            return KernelServiceStatus::InvalidHandle;
        };
        if record.kind != ObjectKind::Channel || !record.has_rights(RIGHT_SET_POLICY) {
            return KernelServiceStatus::AccessDenied;
        }
        let Some(channel_object) = self.channels.get_mut(record.object_id) else {
            return KernelServiceStatus::InvalidHandle;
        };
        channel_object.policy = policy;
        KernelServiceStatus::Ok
    }

    pub fn write_message(
        &mut self,
        channel: Handle,
        bytes: &[u8],
        handles: &[Handle],
    ) -> KernelServiceStatus {
        let Some(record) = self.handles.get(channel.raw) else {
            return KernelServiceStatus::InvalidHandle;
        };
        if record.kind != ObjectKind::Channel || !record.has_rights(RIGHT_WRITE) {
            return KernelServiceStatus::AccessDenied;
        }
        for handle in handles {
            let Some(handle_record) = self.handles.get(handle.raw) else {
                return KernelServiceStatus::InvalidHandle;
            };
            if !handle_record.has_rights(RIGHT_TRANSFER) {
                return KernelServiceStatus::AccessDenied;
            }
        }
        let Some(endpoint) = record.endpoint else {
            return KernelServiceStatus::InvalidHandle;
        };
        let message = match TransferredMessage::new(bytes, handles) {
            Ok(message) => message,
            Err(status) => return status,
        };
        let Some(channel_object) = self.channels.get_mut(record.object_id) else {
            return KernelServiceStatus::InvalidHandle;
        };
        let status = match endpoint {
            Endpoint::A if !channel_object.peer_b_open => KernelServiceStatus::PeerClosed,
            Endpoint::B if !channel_object.peer_a_open => KernelServiceStatus::PeerClosed,
            Endpoint::A => match channel_object.to_b.push(message) {
                Ok(()) => KernelServiceStatus::Ok,
                Err(status) => status,
            },
            Endpoint::B => match channel_object.to_a.push(message) {
                Ok(()) => KernelServiceStatus::Ok,
                Err(status) => status,
            },
        };
        if status == KernelServiceStatus::Ok {
            self.handles.add_signals_for_object(
                record.object_id,
                ObjectKind::Channel,
                SIGNAL_READABLE,
            );
            self.wake_handle_waiters_for_object(
                record.object_id,
                ObjectKind::Channel,
                SIGNAL_READABLE,
            );
        }
        status
    }

    pub fn read_message(
        &mut self,
        channel: Handle,
        max_bytes: usize,
        max_handles: usize,
    ) -> ReadMessageResult {
        let Some(record) = self.handles.get(channel.raw) else {
            return read_status(KernelServiceStatus::InvalidHandle);
        };
        if record.kind != ObjectKind::Channel || !record.has_rights(RIGHT_READ) {
            return read_status(KernelServiceStatus::AccessDenied);
        }
        let Some(endpoint) = record.endpoint else {
            return read_status(KernelServiceStatus::InvalidHandle);
        };
        let Some(channel_object) = self.channels.get_mut(record.object_id) else {
            return read_status(KernelServiceStatus::InvalidHandle);
        };
        let queue = match endpoint {
            Endpoint::A if !channel_object.peer_b_open && channel_object.to_a.is_empty() => {
                return read_status(KernelServiceStatus::PeerClosed);
            }
            Endpoint::B if !channel_object.peer_a_open && channel_object.to_b.is_empty() => {
                return read_status(KernelServiceStatus::PeerClosed);
            }
            Endpoint::A => &mut channel_object.to_a,
            Endpoint::B => &mut channel_object.to_b,
        };
        let Some(front) = queue.front() else {
            return read_status(KernelServiceStatus::TimedOut);
        };
        if front.bytes.len() > max_bytes || front.handles.len() > max_handles {
            return read_status(KernelServiceStatus::BufferTooSmall);
        }
        let Ok(message) = queue.pop() else {
            return read_status(KernelServiceStatus::TimedOut);
        };
        if queue.is_empty() {
            self.handles.remove_signals_for_object(
                record.object_id,
                ObjectKind::Channel,
                SIGNAL_READABLE,
            );
        }
        self.scratch.bytes.clear();
        self.scratch.handles.clear();
        if self
            .scratch
            .bytes
            .try_reserve_exact(message.bytes.len())
            .is_err()
            || self
                .scratch
                .handles
                .try_reserve_exact(message.handles.len())
                .is_err()
        {
            return read_status(KernelServiceStatus::NoMemory);
        }
        self.scratch.bytes.extend_from_slice(&message.bytes);
        self.scratch.handles.extend_from_slice(&message.handles);
        self.handles
            .add_signals_for_object(record.object_id, ObjectKind::Channel, SIGNAL_WRITABLE);
        self.wake_handle_waiters_for_object(record.object_id, ObjectKind::Channel, SIGNAL_WRITABLE);
        ReadMessageResult {
            status: KernelServiceStatus::Ok,
            bytes_len: message.bytes.len(),
            handles_len: message.handles.len(),
        }
    }

    fn deliver_message(
        &mut self,
        message: TransferredMessage,
        max_bytes: usize,
        max_handles: usize,
    ) -> ReadMessageResult {
        if message.bytes.len() > max_bytes || message.handles.len() > max_handles {
            return read_status(KernelServiceStatus::BufferTooSmall);
        }
        self.scratch.bytes.clear();
        self.scratch.handles.clear();
        if self
            .scratch
            .bytes
            .try_reserve_exact(message.bytes.len())
            .is_err()
            || self
                .scratch
                .handles
                .try_reserve_exact(message.handles.len())
                .is_err()
        {
            return read_status(KernelServiceStatus::NoMemory);
        }
        self.scratch.bytes.extend_from_slice(&message.bytes);
        self.scratch.handles.extend_from_slice(&message.handles);
        ReadMessageResult {
            status: KernelServiceStatus::Ok,
            bytes_len: message.bytes.len(),
            handles_len: message.handles.len(),
        }
    }
}

const fn read_status(status: KernelServiceStatus) -> ReadMessageResult {
    ReadMessageResult {
        status,
        bytes_len: 0,
        handles_len: 0,
    }
}

const fn deadline_expired(now_nanos: u64, deadline_nanos: i64) -> bool {
    deadline_nanos == 0 || (deadline_nanos > 0 && now_nanos >= deadline_nanos as u64)
}
use alloc::collections::VecDeque;
use alloc::vec::Vec;
