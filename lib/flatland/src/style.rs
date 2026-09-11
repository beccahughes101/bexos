//! Bounded logical CSS inputs. Computed values belong to rebuildable caches.
use crate::Error;
use alloc::string::String;
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Properties {
    pub identifier: String,
    pub classes: String,
    pub inline: String,
}
impl Properties {
    pub fn bytes(&self) -> usize {
        self.identifier.len() + self.classes.len() + self.inline.len()
    }
    pub fn enabled(&self) -> bool {
        self.bytes() != 0
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.identifier.len() > 64
            || self.classes.len() > 128
            || self.inline.len() > 512
            || self.identifier.chars().any(char::is_whitespace)
            || [&self.identifier, &self.classes, &self.inline]
                .iter()
                .any(|s| s.contains('\0'))
        {
            return Err(Error::Bounds);
        }
        Ok(())
    }
}
