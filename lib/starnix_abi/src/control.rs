use crate::{CONTROL_MAGIC, Error, wire};
use alloc::vec::Vec;

/// Bounded messages sent over the retained appd-to-runner control channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Control {
    Signal(u32),
}

impl Control {
    pub fn encode(self) -> Vec<u8> {
        let mut out = CONTROL_MAGIC.to_vec();
        wire::number(&mut out, 1, 1);
        match self {
            Self::Signal(signal) => wire::number(&mut out, 2, u64::from(signal)),
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = wire::Reader(
            bytes
                .strip_prefix(CONTROL_MAGIC)
                .ok_or(Error::InvalidEncoding)?,
        );
        let (mut version, mut signal) = (0, None);
        while let Some((field, value)) = reader.field()? {
            match field {
                1 => version = value.number()?,
                2 => {
                    signal =
                        Some(u32::try_from(value.number()?).map_err(|_| Error::InvalidEncoding)?)
                }
                _ => {}
            }
        }
        if version != 1 {
            return Err(Error::InvalidEncoding);
        }
        let signal = signal.ok_or(Error::InvalidEncoding)?;
        if signal == 0 || signal > 64 {
            return Err(Error::InvalidOptions);
        }
        Ok(Self::Signal(signal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_round_trips_and_is_bounded() {
        let encoded = Control::Signal(15).encode();
        assert_eq!(Control::decode(&encoded), Ok(Control::Signal(15)));
        assert_eq!(
            Control::decode(&Control::Signal(64).encode()),
            Ok(Control::Signal(64))
        );
        assert!(Control::decode(&Control::Signal(1).encode()[..8]).is_err());
    }
}
