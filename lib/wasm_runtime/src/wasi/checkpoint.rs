//! Explicit resource rebinding for application checkpoints. Canonical ABI
//! handles belong to a component instance and cannot be copied to a new one.
//! Guests save typed tokens at checkpoint time and adopt each token in restore.
use super::{sockets::*, state::*};
use crate::context::Context;
use std::collections::BTreeMap;
use wasmtime::{Result, bail, component::Resource};
#[derive(Clone)]
pub enum Saved {
    Descriptor(Descriptor),
    Input(Input),
    Output(Output),
    Pollable(Pollable),
    DirectoryStream(DirectoryStream),
    IoError(IoError),
    Network(Network),
    Tcp(Tcp),
    Udp(Udp),
    Incoming(Incoming),
    Outgoing(Outgoing),
    Addresses(Addresses),
    TerminalInput,
    TerminalOutput,
}
impl State {
    pub fn snapshot(&mut self) -> Result<BTreeMap<u32, Saved>> {
        if !self.restored.is_empty() {
            bail!("unclaimed restored WASI resources");
        }
        let mut entries = BTreeMap::new();
        for id in &self.ids {
            let value = self.table.get_any_mut(*id)?;
            macro_rules! capture { ($($ty:ident),*) => {$(
                if let Some(value) = value.downcast_ref::<$ty>() {
                    entries.insert(*id, Saved::$ty(value.clone())); continue;
                }
            )*}; }
            capture!(
                Descriptor,
                Input,
                Output,
                Pollable,
                DirectoryStream,
                IoError,
                Network,
                Tcp,
                Udp,
                Incoming,
                Outgoing,
                Addresses
            );
            let saved = if value.is::<TerminalInput>() {
                Saved::TerminalInput
            } else if value.is::<TerminalOutput>() {
                Saved::TerminalOutput
            } else {
                bail!("unsupported live WASI resource")
            };
            entries.insert(*id, saved);
        }
        let mut outputs = BTreeMap::new();
        for value in entries.values_mut() {
            if let Saved::Output(output) | Saved::Pollable(Pollable::Output(output)) = value {
                let captured = if let Some(captured) = outputs.get(&output.id()) {
                    captured
                } else {
                    let captured = Output::restore(output.id(), output.snapshot())?;
                    outputs.entry(output.id()).or_insert(captured)
                };
                *output = captured.clone();
            }
        }
        Ok(entries)
    }
    pub fn restore(&mut self, entries: BTreeMap<u32, Saved>, maximum: usize) -> Result<()> {
        if self.count != 0 || entries.len() > maximum {
            bail!("invalid WASI restore");
        }
        if self
            .budget
            .as_ref()
            .is_some_and(|b| !b.charge_handles(entries.len()))
        {
            bail!("aggregate WASI restore limit");
        }
        self.count = entries.len();
        self.restored = entries;
        Ok(())
    }

    /// Ambient stdio objects are recreated by a fresh component instance and
    /// have no external identity to transfer. Rust's standard library may keep
    /// these objects cached even though the application's logical checkpoint
    /// cannot refer to them. Discard only stateless, unclaimed ambient objects;
    /// files, sockets, buffered output, and application-owned pollables remain
    /// subject to the strict adoption check.
    pub(crate) fn discard_unclaimed_ambient_io(&mut self) {
        let before = self.restored.len();
        self.restored.retain(|_, saved| {
            let ambient = match saved {
                Saved::Input(Input::Empty) | Saved::TerminalInput | Saved::TerminalOutput => true,
                Saved::Output(output) => {
                    let state = output.snapshot();
                    matches!(state.target, super::output_state::Target::Log)
                        && state.pending.is_empty()
                        && state.permit == 0
                        && !state.flushing
                        && state.failure.is_none()
                }
                _ => false,
            };
            !ambient
        });
        let discarded = before - self.restored.len();
        if discarded == 0 {
            return;
        }
        self.count -= discarded;
        self.budget
            .as_ref()
            .expect("instance handle budget")
            .release_handles(discarded);
    }
}
impl Context {
    fn identify<T: Send + 'static>(&mut self, r: Resource<T>) -> Result<u32> {
        self.wasi.table.get(&r)?;
        Ok(r.rep())
    }
    fn adopted<T: Send + 'static>(&mut self, value: T) -> Result<Resource<T>> {
        // Already charged when the candidate's checkpoint was installed.
        let resource = self.wasi.table.push(value)?;
        self.wasi.ids.insert(resource.rep());
        Ok(resource)
    }
}
macro_rules! resource_hooks {
    ($($identify:ident, $adopt:ident, $ty:ident);* $(;)?) => {$(
        fn $identify(&mut self, resource: Resource<$ty>) -> Result<u32> {self.identify(resource)}
        fn $adopt(&mut self, token:u32) -> Result<Result<Resource<$ty>,()>> {
            if !self.restoring { return Ok(Err(())); }
            if !matches!(self.wasi.restored.get(&token),Some(Saved::$ty(_))) {return Ok(Err(()));}
            let Some(Saved::$ty(value))=self.wasi.restored.remove(&token) else {unreachable!()};
            Ok(Ok(self.adopted(value)?))
        }
    )*};
}
impl crate::bindings::bexos::wasm::checkpoint_resources::Host for Context {
    resource_hooks! {
        identify_descriptor, adopt_descriptor, Descriptor;
        identify_input, adopt_input, Input;
        identify_output, adopt_output, Output;
        identify_pollable, adopt_pollable, Pollable;
        identify_directory_stream, adopt_directory_stream, DirectoryStream;
        identify_error, adopt_error, IoError;
        identify_network, adopt_network, Network;
        identify_tcp, adopt_tcp, Tcp;
        identify_udp, adopt_udp, Udp;
        identify_incoming, adopt_incoming, Incoming;
        identify_outgoing, adopt_outgoing, Outgoing;
        identify_addresses, adopt_addresses, Addresses;
    }
}
