use crate::{
    bindings::bexos::wasm::{kernel, ui},
    context::Context,
    resources::{Kind, READ, WRITE},
};
use wasmtime::{Result, bail, component::HasData};
impl HasData for Context {
    type Data<'a> = &'a mut Context;
}
impl kernel::Host for Context {
    fn export_directory(
        &mut self,
        directory: wasmtime::component::Resource<crate::wasi::state::Descriptor>,
    ) -> Result<Result<u32, ()>> {
        if self.restoring {
            bail!("directory export during restore");
        }
        let entry = self.wasi.table.get(&directory)?.entry.clone();
        if entry.handle.kind() != Kind::Directory
            || entry.handle.rights() & crate::resources::TRANSFER == 0
        {
            return Ok(Err(()));
        }
        let host = self.host.clone();
        Ok(self
            .resources
            .receive(1, || {
                let handle = host.duplicate_handle(&*entry.handle)?;
                if handle.kind() != Kind::Directory || handle.rights() != entry.handle.rights() {
                    bail!("invalid duplicated directory");
                }
                Ok(((), vec![handle]))
            })
            .map(|(_, ids)| ids[0])
            .map_err(|_| ()))
    }

    fn channel_pair(&mut self) -> Result<Result<(u32, u32), ()>> {
        if self.restoring {
            bail!("channel creation during restore");
        }
        let host = self.host.clone();
        Ok(self
            .resources
            .receive(2, || {
                let (a, b) = host.channel_pair()?;
                if a.kind() != Kind::Channel || b.kind() != Kind::Channel {
                    bail!("invalid channel pair");
                }
                Ok(((), vec![a, b]))
            })
            .map(|(_, ids)| (ids[0], ids[1]))
            .map_err(|_| ()))
    }
    fn resource_clone(&mut self, id: u32) -> Result<Result<u32, ()>> {
        if self.restoring {
            bail!("resource duplication during restore");
        }
        let entry = self.resources.transferable(id)?.clone();
        let host = self.host.clone();
        Ok(self
            .resources
            .receive(1, || {
                let h = host.duplicate_handle(&*entry.handle)?;
                if h.kind() != entry.handle.kind() || h.rights() != entry.handle.rights() {
                    bail!("invalid duplicated resource");
                }
                Ok(((), vec![h]))
            })
            .map(|(_, ids)| ids[0])
            .map_err(|_| ()))
    }

    fn checkpoint_defer(&mut self) -> Result<()> {
        self.checkpoint_deferred = true;
        Ok(())
    }
    fn socket_pair(&mut self) -> Result<Result<(u32, u32), ()>> {
        Ok(self.terminal_socket_pair().map_err(|_| ()))
    }
    fn socket_read(
        &mut self,
        id: u32,
        max_bytes: u32,
    ) -> Result<Result<Vec<u8>, kernel::StreamError>> {
        self.terminal_socket_read(id, max_bytes as usize)
    }
    fn socket_write(
        &mut self,
        id: u32,
        bytes: Vec<u8>,
    ) -> Result<Result<u32, kernel::StreamError>> {
        self.terminal_socket_write(id, &bytes)
    }
    fn socket_ready(&mut self, id: u32, write: bool) -> Result<Result<bool, ()>> {
        Ok(self.terminal_socket_ready(id, write).map_err(|_| ()))
    }
    fn socket_shutdown(&mut self, id: u32, read: bool, write: bool) -> Result<Result<(), ()>> {
        Ok(self
            .terminal_socket_shutdown(id, read, write)
            .map_err(|_| ()))
    }

    fn inspect_resource(&mut self, id: u32) -> Result<Option<kernel::ResourceInfo>> {
        Ok(self
            .resources
            .entries()
            .find(|(key, _)| *key == id)
            .map(|(_, entry)| kernel::ResourceInfo {
                kind: match entry.handle.kind() {
                    Kind::Channel => kernel::ResourceKind::Channel,
                    Kind::Directory => kernel::ResourceKind::Directory,
                    Kind::File => kernel::ResourceKind::File,
                    Kind::Socket => kernel::ResourceKind::Socket,
                    Kind::Opaque => kernel::ResourceKind::Opaque,
                },
                rights: entry.handle.rights(),
            }))
    }
    fn resource_find(&mut self, name: String) -> Result<Option<u32>> {
        if name.len() > 4096 {
            bail!("resource name limit");
        }
        Ok(self.resources.find(&name))
    }
    fn resource_close(&mut self, id: u32) -> Result<Result<(), ()>> {
        Ok(self.resources.remove(id).map(|_| ()).map_err(|_| ()))
    }
    fn monotonic_ns(&mut self) -> Result<u64> {
        Ok(self.host.monotonic_ns())
    }
    fn channel_write(&mut self, id: u32, message: kernel::Message) -> Result<Result<(), ()>> {
        if self.restoring || message.data.len() > 32768 || message.resources.len() > 16 {
            bail!("channel admission limit");
        }
        let channel = self.resources.get(id, Kind::Channel, WRITE)?.handle.clone();
        let mut entries = Vec::new();
        for (i, id) in message.resources.iter().enumerate() {
            if message.resources[..i].contains(id) {
                bail!("duplicate resource transfer");
            }
            entries.push(self.resources.transferable(*id)?.clone());
        }
        if self
            .host
            .channel_write(&*channel, &message.data, &entries)
            .is_err()
        {
            return Ok(Err(()));
        }
        for id in message.resources {
            self.resources.remove(id)?;
        }
        Ok(Ok(()))
    }
    fn channel_read(
        &mut self,
        id: u32,
        max_bytes: u32,
        max_resources: u32,
    ) -> Result<Result<kernel::Message, ()>> {
        Ok(self
            .channel_read_checked(id, max_bytes, max_resources)?
            .map_err(|_| ()))
    }
    fn channel_read_checked(
        &mut self,
        id: u32,
        max_bytes: u32,
        max_resources: u32,
    ) -> Result<Result<kernel::Message, kernel::StreamError>> {
        if self.restoring
            || max_bytes > 32768
            || max_resources > 16
            || max_resources as usize > self.resources.remaining()
        {
            bail!("channel receive limit");
        }
        let channel = self.resources.get(id, Kind::Channel, READ)?.handle.clone();
        let host = self.host.clone();
        let result = self.resources.receive(max_resources as usize, || {
            let (data, handles) =
                host.channel_read(&*channel, max_bytes as usize, max_resources as usize)?;
            if data.len() > max_bytes as usize {
                bail!("invalid host receive response");
            }
            Ok((data, handles))
        });
        Ok(result
            .map(|(data, resources)| kernel::Message { data, resources })
            .map_err(
                |e| match e.downcast_ref::<std::io::Error>().map(std::io::Error::kind) {
                    Some(std::io::ErrorKind::WouldBlock) => kernel::StreamError::WouldBlock,
                    Some(std::io::ErrorKind::BrokenPipe) => kernel::StreamError::Closed,
                    _ => kernel::StreamError::Failed,
                },
            ))
    }
}

impl ui::Host for Context {
    fn set_node_scene(
        &mut self,
        view: u32,
        node: u64,
        batch: Vec<u8>,
    ) -> Result<Result<(), String>> {
        if self.restoring {
            bail!("node scene during restore");
        }
        Ok(self
            .host
            .ui_set_node_scene(view, node, &batch)
            .map_err(|e| e.to_string()))
    }
    fn create_view(
        &mut self,
        flatland_resource: u32,
        display_resource: u32,
        width: u32,
        height: u32,
    ) -> Result<Result<u32, String>> {
        if self.restoring {
            bail!("ui create during restore");
        }
        let flatland = self
            .resources
            .get(flatland_resource, Kind::Channel, READ | WRITE)?
            .handle
            .clone();
        let display = if display_resource == 0 {
            None
        } else {
            Some(
                self.resources
                    .get(display_resource, Kind::Channel, READ | WRITE)?
                    .handle
                    .clone(),
            )
        };
        Ok(self
            .host
            .ui_create_view(&*flatland, display.as_deref(), width, height)
            .map_err(|error| error.to_string()))
    }

    fn configure_view(
        &mut self,
        view: u32,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<Result<(), String>> {
        if self.restoring {
            bail!("ui configure during restore");
        }
        Ok(self
            .host
            .ui_configure_view(view, width, height, scale)
            .map_err(|error| error.to_string()))
    }

    fn register_asset(
        &mut self,
        view: u32,
        asset: u32,
        kind: u32,
        bytes: Vec<u8>,
    ) -> Result<Result<(), String>> {
        if self.restoring || bytes.len() > 4 << 20 {
            bail!("ui asset during restore or over limit");
        }
        Ok(self
            .host
            .ui_register_asset(view, asset, kind, &bytes)
            .map_err(|error| error.to_string()))
    }

    fn release_asset(&mut self, view: u32, asset: u32) -> Result<Result<(), String>> {
        if self.restoring {
            bail!("ui asset release during restore");
        }
        Ok(self
            .host
            .ui_release_asset(view, asset)
            .map_err(|error| error.to_string()))
    }

    fn submit_scene(&mut self, view: u32, batch: Vec<u8>) -> Result<Result<(), String>> {
        if self.restoring || batch.len() > bexos_dioxus_scene::MAX_BATCH_BYTES {
            bail!("ui scene during restore or over limit");
        }
        Ok(self
            .host
            .ui_submit_scene(view, &batch)
            .map_err(|error| error.to_string()))
    }

    fn poll_input(&mut self, view: u32) -> Result<Result<Vec<ui::InputEvent>, String>> {
        Ok(self
            .host
            .ui_poll_input(view)
            .map(|events| {
                events
                    .into_iter()
                    .take(8)
                    .map(|event| ui::InputEvent {
                        kind: event.kind,
                        device: event.device,
                        id: event.id,
                        phase: event.phase,
                        x: event.x,
                        y: event.y,
                        buttons: event.buttons,
                        scroll_x: event.scroll_x,
                        scroll_y: event.scroll_y,
                        code: event.code,
                        key_state: event.key_state,
                        modifiers: event.modifiers,
                        unicode: event.unicode,
                    })
                    .collect()
            })
            .map_err(|error| error.to_string()))
    }

    fn get_presentation_status(
        &mut self,
        view: u32,
    ) -> Result<Result<ui::PresentationStatus, String>> {
        Ok(self
            .host
            .ui_presentation_status(view)
            .map(|status| ui::PresentationStatus {
                accepted_sequence: status.accepted_sequence,
                pending_count: status.pending_count,
                latched_time_ticks: status.latched_time_ticks,
                scene_generation: status.scene_generation,
            })
            .map_err(|error| error.to_string()))
    }

    fn get_active_backend(&mut self, view: u32) -> Result<Result<ui::BackendStatus, String>> {
        Ok(self
            .host
            .ui_active_backend(view)
            .map(|status| ui::BackendStatus {
                backend: status.backend,
                failure: status.failure,
            })
            .map_err(|error| error.to_string()))
    }

    fn close_view(&mut self, view: u32) -> Result<Result<(), String>> {
        if self.restoring {
            bail!("ui close during restore");
        }
        Ok(self
            .host
            .ui_close_view(view)
            .map_err(|error| error.to_string()))
    }
}
