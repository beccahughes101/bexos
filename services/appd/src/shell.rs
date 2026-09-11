//! Stable shell selection and bounded graphical-session checkpoint state.
use crate::manifest::{Manifest, Process, ShellRole, UpdateStrategy};
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

pub const SYSTEM_PACKAGE: &str = "bexos.app.sysui";
pub const USER_PACKAGE: &str = "bexos.app.userui";

/// Only the internal selected-shell launch path supplies an epoch. Manifest
/// permission values and ordinary launch arguments cannot manufacture a grant.
pub fn stamp_grant(values: &mut Vec<String>, role: ShellRole, epoch: Option<u64>) {
    values.retain(|v| !v.starts_with("shell-role:") && !v.starts_with("shell-session:"));
    let Some(epoch) = epoch else { return };
    let (role, epoch) = match role {
        ShellRole::System => (1, 0),
        ShellRole::User => (2, epoch),
        ShellRole::None => return,
    };
    values.push(alloc::format!("shell-role:{role}"));
    values.push(alloc::format!("shell-session:{epoch}"));
}

pub fn entrypoint(manifest: &Manifest, role: ShellRole) -> Option<&Process> {
    let mut entries = manifest.processes.iter().filter(|p| p.shell_role == role);
    let p = entries.next()?;
    (role != ShellRole::None
        && entries.next().is_none()
        && p.service
        && p.lifecycle.update_strategy == UpdateStrategy::HeartTransplant)
        .then_some(p)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Client {
    pub channel: u64,
    pub callback: u64,
    pub package: String,
    pub uid: u64,
    pub epoch: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub initialized: bool,
    pub sysui: String,
    pub userui: String,
    pub uid: u64,
    pub locked: bool,
    pub epoch: u64,
    pub diagnostic: String,
    pub clients: Vec<Client>,
    pub retry_at: u64,
    pub users_channel: u64,
    pub users_awaiting_response: bool,
}
impl Session {
    /// Called only after usersd authenticates. A locked session keeps its identity.
    pub fn authenticated(&mut self, uid: u64) -> Result<bool, Error> {
        if uid == 0 || (self.uid != 0 && self.uid != uid) {
            return Err(Error::InvalidData);
        }
        let fresh = self.uid == 0;
        if fresh {
            self.epoch = self.epoch.checked_add(1).ok_or(Error::Capacity)?;
        }
        self.uid = uid;
        Ok(fresh)
    }
    pub fn end(&mut self) {
        self.uid = 0;
        self.locked = true;
        self.userui.clear();
        self.epoch = self.epoch.saturating_add(1);
    }
    pub fn authorized(&self, package: &str, uid: u64) -> bool {
        (uid == 0 && !self.sysui.is_empty() && package == self.sysui)
            || (uid != 0 && uid == self.uid && !self.userui.is_empty() && package == self.userui)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(3);
        w.word(self.initialized as u64);
        w.text(&self.sysui);
        w.text(&self.userui);
        w.word(self.uid);
        w.word(self.locked as u64);
        w.word(self.epoch);
        w.text(&self.diagnostic);
        w.word(self.retry_at);
        w.word(self.clients.len() as u64);
        for c in &self.clients {
            w.word(c.channel);
            w.word(c.callback);
            w.text(&c.package);
            w.word(c.uid);
            w.word(c.epoch);
        }
        w.word(self.users_channel);
        w.word(self.users_awaiting_response as u64);
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let version = r.word()?;
        if !(1..=3).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        let mut s = Self {
            initialized: r.flag()?,
            sysui: r.text(128)?.to_string(),
            userui: r.text(128)?.to_string(),
            uid: r.word()?,
            locked: r.flag()?,
            epoch: r.word()?,
            diagnostic: r.text(256)?.to_string(),
            retry_at: r.word()?,
            clients: Vec::new(),
            users_channel: 0,
            users_awaiting_response: false,
        };
        for _ in 0..r.count(16)? {
            let c = Client {
                channel: r.word()?,
                callback: if version >= 2 { r.word()? } else { 0 },
                package: r.text(128)?.to_string(),
                uid: r.word()?,
                epoch: r.word()?,
            };
            if c.channel == 0 || s.clients.iter().any(|v| v.channel == c.channel) {
                return Err(Error::InvalidData);
            }
            s.clients.push(c);
        }
        if version >= 3 {
            s.users_channel = r.word()?;
            s.users_awaiting_response = r.flag()?;
            if s.users_awaiting_response && s.users_channel == 0 {
                return Err(Error::InvalidData);
            }
        }
        r.finish()?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn brokered_grants_remove_forged_tags_and_bind_the_user_session() {
        let mut values = alloc::vec![
            "gpu:read".into(),
            "shell-role:1".into(),
            "shell-session:7".into()
        ];
        stamp_grant(&mut values, ShellRole::System, None);
        assert_eq!(values, ["gpu:read"]);
        stamp_grant(&mut values, ShellRole::User, Some(9));
        assert_eq!(values, ["gpu:read", "shell-role:2", "shell-session:9"]);
        stamp_grant(&mut values, ShellRole::System, Some(9));
        assert_eq!(values, ["gpu:read", "shell-role:1", "shell-session:0"]);
        stamp_grant(&mut values, ShellRole::None, Some(9));
        assert_eq!(values, ["gpu:read"]);
    }
    #[test]
    fn authority_is_selected_package_and_uid() {
        let s = Session {
            sysui: SYSTEM_PACKAGE.into(),
            userui: USER_PACKAGE.into(),
            uid: 1000,
            ..Default::default()
        };
        assert!(s.authorized(SYSTEM_PACKAGE, 0));
        assert!(s.authorized(USER_PACKAGE, 1000));
        assert!(!s.authorized(USER_PACKAGE, 0));
        assert!(!s.authorized(USER_PACKAGE, 1001));
        assert!(!s.authorized("other", 1000));
        assert_eq!(Session::decode(&s.encode()).unwrap(), s);
        let mut invalid = s.encode();
        invalid.push(0);
        assert!(Session::decode(&invalid).is_err());
    }
    #[test]
    fn migration_retains_pending_authentication_reply_fence() {
        let s = Session {
            users_channel: 123,
            users_awaiting_response: true,
            ..Default::default()
        };
        assert_eq!(Session::decode(&s.encode()).unwrap(), s);
        let mut legacy = s.encode();
        legacy[..8].copy_from_slice(&2u64.to_le_bytes());
        legacy.truncate(legacy.len() - 16);
        let restored = Session::decode(&legacy).unwrap();
        assert_eq!(restored.users_channel, 0);
        assert!(!restored.users_awaiting_response);
    }
    #[test]
    fn selection_requires_one_migratable_service() {
        let mut m = Manifest::default();
        m.processes.push(Process {
            shell_role: ShellRole::System,
            service: true,
            ..Default::default()
        });
        assert!(entrypoint(&m, ShellRole::System).is_none());
        m.processes[0].lifecycle.update_strategy = UpdateStrategy::HeartTransplant;
        assert!(entrypoint(&m, ShellRole::System).is_some());
        assert!(entrypoint(&m, ShellRole::User).is_none());
        m.processes.push(m.processes[0].clone());
        assert!(entrypoint(&m, ShellRole::System).is_none());
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    #[test]
    fn lock_resumes_identity_and_logout_revokes_old_session() {
        let mut s = Session::default();
        assert!(s.authenticated(0).is_err());
        assert!(s.authenticated(1000).unwrap());
        let epoch = s.epoch;
        s.locked = true;
        assert!(s.authenticated(1001).is_err());
        assert!(!s.authenticated(1000).unwrap());
        assert_eq!(s.epoch, epoch);
        s.end();
        assert_eq!(s.uid, 0);
        assert!(s.epoch > epoch);
        assert!(s.authenticated(1001).unwrap());
    }
}
