//! Retain the immutable VMO, authenticated clients and undelivered generations.
use crate::{
    binding::Client,
    service::{Listener, Runtime, User},
};
use bexos_locale_settings::{Settings, encode};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};
use locale_fidl::{FidlDecode, LocaleSnapshot};
impl State for Runtime {
    fn empty() -> Self {
        Self::default()
    }
    fn keys(&self) -> Vec<u64> {
        core::iter::once(0)
            .chain(self.users.keys().map(|uid| uid + 1))
            .collect()
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        w.word(1);
        if key == 0 {
            for v in [
                self.control.0,
                self.migration.map_or(0, |c| c.0),
                self.preferences.0,
                self.data,
                self.data_len,
                self.data_generation,
            ] {
                w.word(v);
            }
            w.word(self.clients.len() as u64);
            for c in &self.clients {
                w.word(c.channel);
                w.word(c.uid);
                w.word(u64::from(c.startup));
                w.word(c.methods.len() as u64);
                for m in &c.methods {
                    w.word(*m);
                }
            }
            w.word(self.listeners.len() as u64);
            for l in &self.listeners {
                w.word(l.channel);
                w.word(l.uid);
                w.word(l.generation);
            }
        } else {
            let Some(u) = self.users.get(&(key - 1)) else {
                return Ok(None);
            };
            w.word(u.watch);
            let (bytes, _) = u
                .settings
                .with_wire(|settings| {
                    encode(&LocaleSnapshot {
                        generation: u.generation,
                        settings,
                    })
                })
                .map_err(|_| Error::InvalidData)?;
            w.bytes(&bytes);
        }
        Ok(Some(w.finish()))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let Some(bytes) = bytes else {
            if key > 0 {
                self.users.remove(&(key - 1));
                return Ok(());
            }
            return Err(Error::InvalidData);
        };
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        if key == 0 {
            self.control = Channel(r.word()?);
            let migration = r.word()?;
            self.migration = (migration != 0).then_some(Channel(migration));
            self.preferences = Channel(r.word()?);
            self.data = r.word()?;
            self.data_len = r.word()?;
            self.data_generation = r.word()?;
            self.clients.clear();
            for _ in 0..r.count(256)? {
                let channel = r.word()?;
                let uid = r.word()?;
                let startup = r.flag()?;
                let mut methods = Vec::new();
                for _ in 0..r.count(3)? {
                    methods.push(r.word()?);
                }
                self.clients.push(Client {
                    channel,
                    uid,
                    startup,
                    methods,
                });
            }
            self.listeners.clear();
            for _ in 0..r.count(256)? {
                self.listeners.push(Listener {
                    channel: r.word()?,
                    uid: r.word()?,
                    generation: r.word()?,
                });
            }
        } else {
            let watch = r.word()?;
            let snapshot =
                LocaleSnapshot::decode(r.bytes(8192)?, &[]).map_err(|_| Error::InvalidData)?;
            let settings =
                Settings::from_wire(&snapshot.settings).map_err(|_| Error::InvalidData)?;
            self.users.insert(
                key - 1,
                User {
                    watch,
                    generation: snapshot.generation,
                    settings,
                },
            );
        }
        r.finish()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none()
            || self.preferences.0 == 0
            || self.data == 0
            || self.data_generation == 0
            || self.data_len == 0
            || self.data_len > 64 * 1024 * 1024
            || self.users.len() > 256
        {
            return Err(Error::InvalidData);
        }
        for c in &self.clients {
            if c.channel == 0
                || c.methods.iter().any(|m| {
                    if c.startup {
                        *m != 1
                    } else {
                        !matches!(m, 1 | 2 | 3)
                    }
                })
                || (c.startup && c.uid != 0)
            {
                return Err(Error::InvalidData);
            }
        }
        for l in &self.listeners {
            if l.channel == 0 || !self.users.contains_key(&l.uid) || self.users.get(&l.uid).is_some_and(|u| l.generation > u.generation) {
                return Err(Error::InvalidData);
            }
        }
        for (uid, u) in &self.users {
            if *uid == u64::MAX || u.watch == 0 {
                return Err(Error::InvalidData);
            }
        }
        let mut handles=std::collections::BTreeSet::new();
        for handle in [self.control.0,self.preferences.0,self.data,self.migration.unwrap().0].into_iter()
            .chain(self.clients.iter().map(|c|c.channel))
            .chain(self.users.values().map(|u|u.watch))
            .chain(self.listeners.iter().map(|l|l.channel)) {
            if handle==0 || !handles.insert(handle){return Err(Error::InvalidData);}
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        let mut handles = std::collections::BTreeSet::new();
        handles.extend([
            self.control.0,
            self.preferences.0,
            self.data,
            self.migration.map_or(0, |c| c.0),
        ]);
        handles.extend(self.clients.iter().map(|c| c.channel));
        handles.extend(self.users.values().map(|u| u.watch));
        handles.extend(self.listeners.iter().map(|l| l.channel));
        handles.remove(&0);
        handles.into_iter().map(Resource::Handle).collect()
    }
    fn activated(&mut self, _generation: u64) {
        bexos_userspace::log("localed: transplant activated\n");
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transplant_keeps_pending_delivery() {
        let mut r = Runtime::default();
        r.control = Channel(1);
        r.migration = Some(Channel(2));
        r.preferences = Channel(3);
        r.data = 4;
        r.data_len = 4096;
        r.data_generation = 1;
        r.users.insert(
            7,
            User {
                settings: Settings::default(),
                generation: 3,
                watch: 5,
            },
        );
        r.listeners.push(Listener {
            uid: 7,
            channel: 6,
            generation: 2,
        });
        let mut next = Runtime::empty();
        for key in r.keys() {
            let b = r.encode_record(key).unwrap().unwrap();
            next.adopt_record(key, Some(&b)).unwrap();
        }
        next.validate().unwrap();
        assert_eq!(next.users, r.users);
        assert_eq!(next.listeners, r.listeners);
        assert_eq!(next.data, r.data);
        assert_eq!(next.resources().len(), 6);
    }
}
