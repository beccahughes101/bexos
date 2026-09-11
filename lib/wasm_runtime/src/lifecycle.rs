//! Versioned cooperative service hooks. No engine stack or serialized native
//! artifact is admitted as application migration state.
use crate::instance::CoreInstance;
use wasmtime::{Result, bail};
pub const VERSION: u32 = 1;
pub const MAX_CHECKPOINT: usize = 1 << 20;
impl CoreInstance {
    pub fn validate_service(&mut self) -> Result<()> {
        self.instance
            .get_typed_func::<(), u32>(&mut self.store, "bexos-service-version")?;
        self.instance
            .get_typed_func::<u32, ()>(&mut self.store, "bexos-service-dispatch")?;
        self.instance
            .get_typed_func::<(), u64>(&mut self.store, "bexos-checkpoint")?;
        self.instance
            .get_typed_func::<u32, u32>(&mut self.store, "bexos-restore-allocate")?;
        self.instance
            .get_typed_func::<(u32, u32), u32>(&mut self.store, "bexos-restore")?;
        self.instance
            .get_typed_func::<(), ()>(&mut self.store, "bexos-activate")?;
        self.instance
            .get_typed_func::<(), ()>(&mut self.store, "bexos-abort")?;
        Ok(())
    }
    pub async fn service_version(&mut self) -> Result<()> {
        self.validate_service()?;
        let f = self
            .instance
            .get_typed_func::<(), u32>(&mut self.store, "bexos-service-version")?;
        if f.call_async(&mut self.store, ()).await? != VERSION {
            bail!("incompatible service ABI");
        }
        Ok(())
    }
    pub async fn checkpoint(&mut self) -> Result<Vec<u8>> {
        let f = self
            .instance
            .get_typed_func::<(), u64>(&mut self.store, "bexos-checkpoint")?;
        let packed = f.call_async(&mut self.store, ()).await?;
        let pointer = (packed >> 32) as usize;
        let length = (packed as u32) as usize;
        if length > MAX_CHECKPOINT {
            bail!("checkpoint limit");
        }
        let memory = self
            .instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| wasmtime::format_err!("checkpoint memory missing"))?;
        let mut bytes = vec![0; length];
        memory.read(&self.store, pointer, &mut bytes)?;
        Ok(bytes)
    }
    pub async fn restore_checkpoint(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_CHECKPOINT {
            bail!("checkpoint limit");
        }
        self.service_version().await?;
        let allocate = self
            .instance
            .get_typed_func::<u32, u32>(&mut self.store, "bexos-restore-allocate")?;
        let pointer = allocate
            .call_async(&mut self.store, bytes.len() as u32)
            .await?;
        let memory = self
            .instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| wasmtime::format_err!("restore memory missing"))?;
        memory.write(&mut self.store, pointer as usize, bytes)?;
        let restore = self
            .instance
            .get_typed_func::<(u32, u32), u32>(&mut self.store, "bexos-restore")?;
        if restore
            .call_async(&mut self.store, (pointer, bytes.len() as u32))
            .await?
            != 0
        {
            bail!("guest rejected checkpoint");
        }
        Ok(())
    }
    pub async fn activate(&mut self) -> Result<()> {
        self.store.data_mut().restoring = false;
        let f = self
            .instance
            .get_typed_func::<(), ()>(&mut self.store, "bexos-activate")?;
        f.call_async(&mut self.store, ()).await
    }
    pub async fn abort_migration(&mut self) -> Result<()> {
        let f = self
            .instance
            .get_typed_func::<(), ()>(&mut self.store, "bexos-abort")?;
        f.call_async(&mut self.store, ()).await
    }
    pub async fn dispatch(&mut self, resource: u32) -> Result<()> {
        let f = self
            .instance
            .get_typed_func::<u32, ()>(&mut self.store, "bexos-service-dispatch")?;
        f.call_async(&mut self.store, resource).await
    }
}
