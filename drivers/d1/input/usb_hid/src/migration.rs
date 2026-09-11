use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_usb_host::hid::{BootKeyboardReport, BootMouseReport};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
    service_binding::BoundServiceEndpoint,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HidKind {
    #[default]
    Keyboard,
    Mouse,
}

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub lifecycle: Option<Channel>,
    pub interface: Option<Channel>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub sink: Option<Channel>,
    pub node: u64,
    pub kind: HidKind,
    pub sequence: u64,
    pub reports: u64,
    pub reset: bool,
    pub held_keyboard: BootKeyboardReport,
    pub held_mouse: BootMouseReport,
    pub paused: bool,
}

impl Runtime {
    pub fn new(control: Channel, migration: Option<Channel>) -> Self {
        Self {
            control,
            migration,
            lifecycle: None,
            interface: None,
            clients: Vec::new(),
            sink: None,
            node: 0,
            kind: HidKind::Keyboard,
            sequence: 0,
            reports: 0,
            reset: true,
            held_keyboard: BootKeyboardReport::default(),
            held_mouse: BootMouseReport::default(),
            paused: false,
        }
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self::new(Channel(0), None)
    }

    fn keys(&self) -> Vec<u64> {
        alloc::vec![0, 1]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        match key {
            0 => {
                w.word(1);
                w.word(self.control.0);
                w.word(self.migration.map_or(0, |c| c.0));
                w.word(self.lifecycle.map_or(0, |c| c.0));
                w.word(self.interface.map_or(0, |c| c.0));
                w.word(self.sink.map_or(0, |c| c.0));
                w.word(self.node);
                w.word(self.kind as u64);
                w.word(self.sequence);
                w.word(self.reports);
                w.word(self.reset as u64);
                w.word(self.paused as u64);
                w.word(self.held_keyboard.modifiers as u64);
                for key in self.held_keyboard.keys {
                    w.word(key as u64);
                }
                w.word(self.held_mouse.buttons as u64);
                w.word(self.held_mouse.x as i64 as u64);
                w.word(self.held_mouse.y as i64 as u64);
                w.word(self.held_mouse.wheel as i64 as u64);
            }
            1 => {
                w.word(self.clients.len() as u64);
                for client in &self.clients {
                    w.word(client.channel.0);
                    w.word(client.allowed_methods.len() as u64);
                    for ordinal in &client.allowed_methods {
                        w.word(*ordinal);
                    }
                }
            }
            _ => return Err(Error::InvalidData),
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        match key {
            0 => {
                if r.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                self.control = Channel(r.word()?);
                self.migration = Some(Channel(r.word()?));
                self.lifecycle = nonzero(r.word()?);
                self.interface = nonzero(r.word()?);
                self.sink = nonzero(r.word()?);
                self.node = r.word()?;
                self.kind = match r.word()? {
                    0 => HidKind::Keyboard,
                    1 => HidKind::Mouse,
                    _ => return Err(Error::InvalidData),
                };
                self.sequence = r.word()?;
                self.reports = r.word()?;
                self.reset = r.flag()?;
                self.paused = r.flag()?;
                self.held_keyboard.modifiers = r.word()? as u8;
                for key in &mut self.held_keyboard.keys {
                    *key = r.word()? as u8;
                }
                self.held_mouse.buttons = r.word()? as u8;
                self.held_mouse.x = r.word()? as i8;
                self.held_mouse.y = r.word()? as i8;
                self.held_mouse.wheel = r.word()? as i8;
            }
            1 => {
                self.clients.clear();
                for _ in 0..r.count(64)? {
                    let channel = Channel(r.word()?);
                    let mut allowed = Vec::new();
                    for _ in 0..r.count(16)? {
                        allowed.push(r.word()?);
                    }
                    self.clients
                        .push(BoundServiceEndpoint::new(channel, allowed));
                }
            }
            _ => return Err(Error::InvalidData),
        }
        r.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn quiescence_ready(&self) -> bool {
        true
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.control.0)];
        for channel in [self.migration, self.lifecycle, self.interface, self.sink]
            .into_iter()
            .flatten()
        {
            out.push(Resource::Handle(channel.0));
        }
        out.extend(
            self.clients
                .iter()
                .map(|client| Resource::Handle(client.channel.0)),
        );
        out
    }

    fn activated(&mut self, generation: u64) {
        self.paused = false;
        bexos_userspace::log(&alloc::format!(
            "usb-hid: adopted generation={generation}\n"
        ));
    }
}

fn nonzero(handle: u64) -> Option<Channel> {
    (handle != 0).then_some(Channel(handle))
}
