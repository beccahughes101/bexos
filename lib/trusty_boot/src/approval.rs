//! The boot owner must finish this exchange before entering normal-world code.
//! Reading a rollback index alone is not approval: the device must be locked,
//! the image must satisfy the floor, and Trusty must seal boot-only mutations.
use crate::{
    avb::Avb,
    ql::{Error as TransportError, Transport},
};
#[path = "approval_wire.rs"]
mod wire;
pub use wire::STATE_BYTES;

pub trait State {
    fn locked(&mut self) -> Result<bool, TransportError>;
    fn floor(&mut self, location: u32) -> Result<u64, TransportError>;
    fn seal(&mut self) -> Result<(), TransportError>;
}
impl<T: Transport> State for Avb<T> {
    fn locked(&mut self) -> Result<bool, TransportError> {
        self.read_lock_state()
    }
    fn floor(&mut self, location: u32) -> Result<u64, TransportError> {
        self.read_rollback(location)
    }
    fn seal(&mut self) -> Result<(), TransportError> {
        self.lock_boot_state()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidGeneration,
    Unlocked,
    Rollback,
    Transport(TransportError),
}
impl From<TransportError> for Error {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Approval {
    generation: u64,
    floor: u64,
    location: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Generation {
    pub generation: u64,
    pub location: u32,
}
impl Approval {
    pub fn generation(self) -> u64 {
        self.generation
    }
    pub fn floor(self) -> u64 {
        self.floor
    }
    pub fn location(self) -> u32 {
        self.location
    }
}
/// This does not raise the persistent floor. Replacement transactions advance
/// it only at their durable commit boundary, after candidate health succeeds.
pub fn approve(state: &mut impl State, generation: u64, location: u32) -> Result<Approval, Error> {
    Ok(approve_chain(
        state,
        [Generation {
            generation,
            location,
        }],
    )?[0])
}

/// Check every authenticated execution component before sealing boot state.
/// Distinct rollback locations prevent one component from approving another.
/// Neither boot approval nor candidate staging raises any persistent floor.
pub fn approve_chain<const N: usize>(
    state: &mut impl State,
    images: [Generation; N],
) -> Result<[Approval; N], Error> {
    if N == 0 || N > 4 {
        return Err(Error::InvalidGeneration);
    }
    for (index, image) in images.iter().enumerate() {
        if image.generation == 0
            || image.location > 31
            || images[..index]
                .iter()
                .any(|earlier| earlier.location == image.location)
        {
            return Err(Error::InvalidGeneration);
        }
    }
    if !state.locked()? {
        return Err(Error::Unlocked);
    }
    let mut approvals = [Approval {
        generation: 0,
        floor: 0,
        location: 0,
    }; N];
    for (image, approved) in images.iter().zip(&mut approvals) {
        let floor = state.floor(image.location)?;
        if image.generation < floor {
            return Err(Error::Rollback);
        }
        *approved = Approval {
            generation: image.generation,
            floor,
            location: image.location,
        };
    }
    state.seal()?;
    Ok(approvals)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Device {
        locked: bool,
        floor: u64,
        fault: u8,
        calls: u8,
    }
    impl State for Device {
        fn locked(&mut self) -> Result<bool, TransportError> {
            self.calls |= 1;
            if self.fault == 1 {
                return Err(TransportError::InvalidResponse);
            }
            Ok(self.locked)
        }
        fn floor(&mut self, location: u32) -> Result<u64, TransportError> {
            assert_eq!(location, 3);
            self.calls |= 2;
            if self.fault == 2 {
                return Err(TransportError::Rejected);
            }
            Ok(self.floor)
        }
        fn seal(&mut self) -> Result<(), TransportError> {
            self.calls |= 4;
            if self.fault == 4 {
                return Err(TransportError::Timeout);
            }
            Ok(())
        }
    }
    #[test]
    fn approval_requires_locked_fresh_image_and_acknowledged_seal() {
        for (locked, floor, fault, result, calls) in [
            (
                true,
                7,
                0,
                Ok(Approval {
                    generation: 7,
                    floor: 7,
                    location: 3,
                }),
                7,
            ),
            (
                true,
                6,
                0,
                Ok(Approval {
                    generation: 7,
                    floor: 6,
                    location: 3,
                }),
                7,
            ),
            (false, 0, 0, Err(Error::Unlocked), 1),
            (true, 8, 0, Err(Error::Rollback), 3),
            (
                true,
                7,
                1,
                Err(Error::Transport(TransportError::InvalidResponse)),
                1,
            ),
            (
                true,
                7,
                2,
                Err(Error::Transport(TransportError::Rejected)),
                3,
            ),
            (
                true,
                7,
                4,
                Err(Error::Transport(TransportError::Timeout)),
                7,
            ),
        ] {
            let mut device = Device {
                locked,
                floor,
                fault,
                calls: 0,
            };
            assert_eq!(approve(&mut device, 7, 3), result);
            assert_eq!(device.calls, calls);
            assert_eq!(device.floor, floor);
        }
    }
    #[test]
    fn invalid_metadata_cannot_reach_persistent_state() {
        let mut device = Device {
            locked: true,
            floor: 0,
            fault: 0,
            calls: 0,
        };
        assert_eq!(approve(&mut device, 0, 3), Err(Error::InvalidGeneration));
        assert_eq!(approve(&mut device, 7, 32), Err(Error::InvalidGeneration));
        assert_eq!(device.calls, 0);
    }

    #[test]
    fn all_component_floors_must_pass_before_one_seal_without_mutating_storage() {
        struct Chain {
            floors: [u64; 32],
            reads: u32,
            seals: u32,
        }
        impl State for Chain {
            fn locked(&mut self) -> Result<bool, TransportError> {
                Ok(true)
            }
            fn floor(&mut self, location: u32) -> Result<u64, TransportError> {
                self.reads |= 1 << location;
                Ok(self.floors[location as usize])
            }
            fn seal(&mut self) -> Result<(), TransportError> {
                self.seals += 1;
                Ok(())
            }
        }
        let images = [
            Generation {
                generation: 5,
                location: 0,
            },
            Generation {
                generation: 2,
                location: 30,
            },
            Generation {
                generation: 3,
                location: 31,
            },
        ];
        let mut state = Chain {
            floors: [0; 32],
            reads: 0,
            seals: 0,
        };
        state.floors[31] = 4;
        assert_eq!(approve_chain(&mut state, images), Err(Error::Rollback));
        assert_eq!(state.reads, 0xc0000001);
        assert_eq!(state.seals, 0);
        state.floors[31] = 3;
        let approved = approve_chain(&mut state, images).unwrap();
        assert_eq!(state.seals, 1);
        assert_eq!(approved.map(Approval::generation), [5, 2, 3]);
        assert_eq!(approved.map(Approval::floor), [0, 0, 3]);
        assert_eq!(state.floors[31], 3);
        let reads = state.reads;
        assert_eq!(
            approve_chain(&mut state, [images[0], images[0]]),
            Err(Error::InvalidGeneration)
        );
        assert_eq!(state.reads, reads);
        assert_eq!(state.seals, 1);
    }
}
