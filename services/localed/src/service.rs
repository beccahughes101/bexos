use crate::binding::Client;
use bexos_locale_settings::Settings;
use bexos_userspace::Channel;
use std::collections::BTreeMap;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct User {
    pub settings: Settings,
    pub generation: u64,
    pub watch: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Listener {
    pub channel: u64,
    pub uid: u64,
    pub generation: u64,
}
pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub preferences: Channel,
    pub data: u64,
    pub data_len: u64,
    pub data_generation: u64,
    pub clients: Vec<Client>,
    pub users: BTreeMap<u64, User>,
    pub listeners: Vec<Listener>,
    /// Preference-watch replies that may arrive after startup used defaults.
    pub pending_initial: Vec<u64>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            preferences: Channel(0),
            data: 0,
            data_len: 0,
            data_generation: 0,
            clients: Vec::new(),
            users: BTreeMap::new(),
            listeners: Vec::new(),
            pending_initial: Vec::new(),
        }
    }
}
impl Runtime {
    pub fn update(&mut self, uid: u64, generation: u64, settings: Settings) -> bool {
        if let Some(user) = self.users.get_mut(&uid) {
            if generation > user.generation {
                user.generation = generation;
                user.settings = settings;
                return true;
            }
        }
        false
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revisions_are_monotonic_and_uid_scoped() {
        let mut r = Runtime::default();
        for uid in [1, 2] {
            r.users.insert(
                uid,
                User {
                    settings: Settings::default(),
                    generation: 2,
                    watch: uid,
                },
            );
        }
        let s = Settings {
            region: "de-DE".into(),
            ..Settings::default()
        };
        assert!(!r.update(1, 1, s.clone()));
        assert!(r.update(1, 3, s));
        assert_eq!(r.users[&2].settings.region, "en-US");
    }
}
