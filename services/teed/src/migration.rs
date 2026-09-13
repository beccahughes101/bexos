extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Channel;
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::service_binding::BoundServiceEndpoint;
use tee_manager_fidl::{TeeUpdatePhase, TeeUpdateStatus};

use crate::{
    DriverBackend, SoftwareEmuBackend, TeeBackend, TeeService, TeeUpdateProgress, TrustedApp,
};

pub struct Runtime<B: TeeBackend = DriverBackend> {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub service: TeeService<B>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub generation: u64,
    pub session_owners: alloc::collections::BTreeMap<u64, u64>,
}

impl<B: TeeBackend> Runtime<B> {
    pub fn new(control: Channel, migration: Option<Channel>, service: TeeService<B>) -> Self {
        Self {
            control,
            migration,
            service,
            clients: Vec::new(),
            session_owners: Default::default(),
            generation: 1,
        }
    }
}

pub trait MigratableBackend: TeeBackend {
    fn snapshot(&self) -> SoftwareEmuBackend;
    fn restore(&mut self, backend: SoftwareEmuBackend);
}

impl MigratableBackend for SoftwareEmuBackend {
    fn snapshot(&self) -> SoftwareEmuBackend {
        self.clone()
    }

    fn restore(&mut self, backend: SoftwareEmuBackend) {
        *self = backend;
    }
}

impl MigratableBackend for DriverBackend {
    fn snapshot(&self) -> SoftwareEmuBackend {
        self.snapshot_state()
    }

    fn restore(&mut self, backend: SoftwareEmuBackend) {
        self.restore_state(backend);
    }
}

impl<B: MigratableBackend + Default> State for Runtime<B> {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            service: TeeService::new(B::default()),
            clients: Vec::new(),
            session_owners: Default::default(),
            generation: 0,
        }
    }

    fn keys(&self) -> Vec<u64> {
        alloc::vec![0]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let backend = self.service.backend().snapshot();
        let (apps, sessions, next_session_id, secure_os_version, anti_rollback_version, update) =
            backend.parts();
        let mut w = Encoder::new();
        w.word(6);
        w.word(self.control.0);
        w.word(self.migration.map_or(0, |c| c.0));
        w.word(self.generation);
        w.word(next_session_id);
        w.word(secure_os_version as u64);
        w.word(anti_rollback_version as u64);
        encode_update(&mut w, update);
        w.word(apps.len() as u64);
        for app in apps {
            encode_app(&mut w, app);
        }
        w.word(sessions.len() as u64);
        for (id, uuid, port) in sessions {
            w.word(id);
            w.bytes(&uuid);
            w.text(port.as_deref().unwrap_or_default());
        }
        w.word(self.clients.len() as u64);
        for client in &self.clients {
            w.word(client.channel.0);
            w.word(client.allowed_methods.len() as u64);
            for ordinal in &client.allowed_methods {
                w.word(*ordinal);
            }
        }
        w.word(self.service.backend().rpmb_channel().map_or(0, |c| c.0));
        w.word(self.session_owners.len() as u64);
        for (session, owner) in &self.session_owners {
            w.word(*session);
            w.word(*owner);
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        let version = r.word()?;
        if !(1..=6).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        self.control = Channel(r.word()?);
        self.migration = Some(Channel(r.word()?));
        self.generation = r.word()?;
        let next_session_id = r.word()?;
        let secure_os_version = r.word()? as u32;
        let anti_rollback_version = r.word()? as u32;
        let update = decode_update(&mut r, version)?;
        let mut apps = Vec::new();
        for _ in 0..r.count(32)? {
            apps.push(decode_app(&mut r, version)?);
        }
        let mut sessions = Vec::new();
        for _ in 0..r.count(1024)? {
            let id = r.word()?;
            let uuid: [u8; 16] = r.bytes(16)?.try_into().map_err(|_| Error::InvalidData)?;
            let port = if version >= 4 {
                let raw = r.text(128)?;
                if raw.is_empty() {
                    None
                } else {
                    Some(String::from(raw))
                }
            } else {
                None
            };
            sessions.push((id, uuid, port));
        }
        self.clients.clear();
        for _ in 0..r.count(64)? {
            let channel = Channel(r.word()?);
            let mut allowed_methods = Vec::new();
            for _ in 0..r.count(64)? {
                allowed_methods.push(r.word()?);
            }
            self.clients
                .push(BoundServiceEndpoint::new(channel, allowed_methods));
        }
        let rpmb = if version >= 5 { r.word()? } else { 0 };
        self.session_owners.clear();
        if version >= 6 {
            for _ in 0..r.count(1024)? {
                let session = r.word()?;
                let owner = r.word()?;
                if !sessions.iter().any(|(id, _, _)| *id == session)
                    || !self.clients.iter().any(|client| client.channel.0 == owner)
                    || self.session_owners.insert(session, owner).is_some()
                {
                    return Err(Error::InvalidData);
                }
            }
        }
        r.finish()?;
        if rpmb != 0 {
            self.service.backend_mut().set_rpmb_channel(Channel(rpmb));
        }
        self.service
            .backend_mut()
            .restore(SoftwareEmuBackend::from_parts(
                apps,
                sessions,
                next_session_id,
                secure_os_version,
                anti_rollback_version,
                update,
            ));
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() || self.generation == 0 {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.control.0)];
        if let Some(channel) = self.migration {
            out.push(Resource::Handle(channel.0));
        }
        out.extend(
            self.clients
                .iter()
                .map(|client| Resource::Handle(client.channel.0)),
        );
        if let Some(channel) = self.service.backend().rpmb_channel() {
            out.push(Resource::Handle(channel.0));
        }
        out
    }

    fn activated(&mut self, generation: u64) {
        if self.service.backend_mut().activate_backend().is_err() {
            bexos_userspace::log("teed: migrated secure transport unavailable; failing closed\n");
            bexos_userspace::exit();
        }
        self.generation = generation;
        bexos_userspace::log(&alloc::format!("teed: adopted generation={generation}\n"));
    }
}

fn encode_app(w: &mut Encoder, app: &TrustedApp) {
    w.bytes(&app.uuid);
    w.word(app.version as u64);
    w.word(app.active_sessions as u64);
    w.text(&app.entry_point_name);
    w.word(app.storage_bytes_used);
    w.text(&app.package_id);
    w.word(app.service_ports.len() as u64);
    for port in &app.service_ports {
        w.text(port);
    }
    w.word(app.protected as u64);
    w.word(app.package_managed as u64);
}

fn decode_app(r: &mut Decoder<'_>, version: u64) -> Result<TrustedApp, Error> {
    let uuid: [u8; 16] = r.bytes(16)?.try_into().map_err(|_| Error::InvalidData)?;
    Ok(TrustedApp {
        uuid,
        version: r.word()? as u32,
        active_sessions: r.word()? as u32,
        entry_point_name: String::from(r.text(64)?),
        storage_bytes_used: r.word()?,
        package_id: if version >= 3 {
            String::from(r.text(128)?)
        } else {
            String::new()
        },
        service_ports: if version >= 3 {
            let mut ports = Vec::new();
            for _ in 0..r.count(16)? {
                ports.push(String::from(r.text(128)?));
            }
            ports
        } else {
            Vec::new()
        },
        protected: if version >= 3 { r.word()? != 0 } else { false },
        package_managed: if version >= 3 { r.word()? != 0 } else { false },
    })
}

fn encode_update(w: &mut Encoder, update: &TeeUpdateProgress) {
    w.word(update.status as u64);
    w.word(update.phase as u64);
    w.text(&update.active_slot);
    w.text(&update.pending_slot);
    w.word(update.generation);
    w.word(update.rollback_available as u64);
    w.word(update.reboot_required as u64);
    w.text(&update.message);
}

fn decode_update(r: &mut Decoder<'_>, version: u64) -> Result<TeeUpdateProgress, Error> {
    let status = match r.word()? {
        1 => TeeUpdateStatus::Idle,
        2 => TeeUpdateStatus::Staged,
        3 => TeeUpdateStatus::Applying,
        4 => TeeUpdateStatus::Completed,
        5 => TeeUpdateStatus::Failed,
        _ => return Err(Error::InvalidData),
    };
    let phase = if version >= 2 {
        match r.word()? {
            1 => TeeUpdatePhase::Idle,
            2 => TeeUpdatePhase::Verifying,
            3 => TeeUpdatePhase::Staged,
            4 => TeeUpdatePhase::LiveSwitch,
            5 => TeeUpdatePhase::RebootPending,
            6 => TeeUpdatePhase::HealthWindow,
            7 => TeeUpdatePhase::Completed,
            8 => TeeUpdatePhase::RolledBack,
            9 => TeeUpdatePhase::Failed,
            10 => TeeUpdatePhase::RecoveryRequired,
            _ => return Err(Error::InvalidData),
        }
    } else {
        TeeUpdatePhase::Idle
    };
    let active_slot = if version >= 2 {
        String::from(r.text(16)?)
    } else {
        "A".into()
    };
    let pending_slot = if version >= 2 {
        String::from(r.text(16)?)
    } else {
        String::new()
    };
    let generation = r.word()?;
    let rollback_available = version >= 2 && r.flag()?;
    let reboot_required = version >= 2 && r.flag()?;
    Ok(TeeUpdateProgress {
        status,
        phase,
        active_slot,
        pending_slot,
        generation,
        rollback_available,
        reboot_required,
        message: String::from(r.text(256)?),
    })
}
