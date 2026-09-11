use super::{
    state::Pollable,
    wasi::{clocks::*, random::*},
};
use crate::context::Context;
use wasmtime::{Result, bail, component::Resource};
// Clock/random WASI is supplied by the standalone runner. Embedded stores have
// no ambient WASI authority; the checked BexOS monotonic hostcall remains
// available for private computation. Delegated filesystem/network resources are
// handled separately by their typed resource tables.
fn standalone(context: &Context) -> Result<()> {
    if context.child_permit.is_some() {
        bail!("ambient WASI clocks and randomness unavailable in child sandbox");
    }
    Ok(())
}
impl monotonic_clock::Host for Context {
    fn now(&mut self) -> Result<u64> {
        standalone(self)?;
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        Ok(self.host.monotonic_ns())
    }
    fn resolution(&mut self) -> Result<u64> {
        standalone(self)?;
        Ok(1)
    }
    fn subscribe_instant(&mut self, when: u64) -> Result<Resource<Pollable>> {
        standalone(self)?;
        self.push(Pollable::Timer(when))
    }
    fn subscribe_duration(&mut self, duration: u64) -> Result<Resource<Pollable>> {
        standalone(self)?;
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        self.push(Pollable::Timer(
            self.host.monotonic_ns().saturating_add(duration),
        ))
    }
}
impl wall_clock::Host for Context {
    fn now(&mut self) -> Result<wall_clock::Datetime> {
        standalone(self)?;
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let ns = self.host.wall_clock_ns()?;
        Ok(wall_clock::Datetime {
            seconds: ns / 1_000_000_000,
            nanoseconds: (ns % 1_000_000_000) as u32,
        })
    }
    fn resolution(&mut self) -> Result<wall_clock::Datetime> {
        standalone(self)?;
        Ok(wall_clock::Datetime {
            seconds: 0,
            nanoseconds: 1,
        })
    }
}
impl random::Host for Context {
    fn get_random_bytes(&mut self, len: u64) -> Result<Vec<u8>> {
        standalone(self)?;
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        if len > 32768 {
            bail!("random request limit");
        }
        self.host.random(len as usize)
    }
    fn get_random_u64(&mut self) -> Result<u64> {
        standalone(self)?;
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let bytes = self.host.random(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
            wasmtime::format_err!("invalid random source response")
        })?))
    }
}
impl insecure::Host for Context {
    fn get_insecure_random_bytes(&mut self, len: u64) -> Result<Vec<u8>> {
        random::Host::get_random_bytes(self, len)
    }
    fn get_insecure_random_u64(&mut self) -> Result<u64> {
        random::Host::get_random_u64(self)
    }
}
impl insecure_seed::Host for Context {
    fn insecure_seed(&mut self) -> Result<(u64, u64)> {
        standalone(self)?;
        if let Some(seed) = self.insecure_seed {
            return Ok(seed);
        }
        // Hash-table construction during restore uses the saved seed and never
        // advances the kernel random stream or performs an external operation.
        let seed = (
            random::Host::get_random_u64(self)?,
            random::Host::get_random_u64(self)?,
        );
        self.insecure_seed = Some(seed);
        Ok(seed)
    }
}
impl timezone::Host for Context {
    fn display(&mut self, _when: wall_clock::Datetime) -> Result<timezone::TimezoneDisplay> {
        Ok(timezone::TimezoneDisplay {
            utc_offset: 0,
            name: "UTC".into(),
            in_daylight_saving_time: false,
        })
    }
    fn utc_offset(&mut self, _when: wall_clock::Datetime) -> Result<i32> {
        Ok(0)
    }
}
