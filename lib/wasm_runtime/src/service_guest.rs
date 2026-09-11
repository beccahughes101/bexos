use crate::{
    context::Context, control::Control, instance::CoreInstance, service_component::ServiceInstance,
};
use std::sync::Arc;
use wasmtime::{Engine, Result, Store};
pub enum ServiceGuest {
    Core(CoreInstance),
    Component(ServiceInstance),
}
impl ServiceGuest {
    pub async fn instantiate(engine: &Engine, bytes: Arc<[u8]>, context: Context) -> Result<Self> {
        if bytes.get(4..8) == Some(&[1, 0, 0, 0]) {
            let mut core = CoreInstance::instantiate(engine, bytes, context).await?;
            core.service_version().await?;
            Ok(Self::Core(core))
        } else {
            Ok(Self::Component(
                ServiceInstance::instantiate(engine, bytes, context).await?,
            ))
        }
    }
    pub fn store(&self) -> &Store<Context> {
        match self {
            Self::Core(i) => &i.store,
            Self::Component(i) => &i.store,
        }
    }
    pub fn store_mut(&mut self) -> &mut Store<Context> {
        match self {
            Self::Core(i) => &mut i.store,
            Self::Component(i) => &mut i.store,
        }
    }
    pub fn control(&self) -> &Arc<Control> {
        match self {
            Self::Core(i) => &i.control,
            Self::Component(i) => &i.control,
        }
    }
    pub fn bytes(&self) -> Arc<[u8]> {
        match self {
            Self::Core(i) => i.bytes.clone(),
            Self::Component(i) => i.bytes.clone(),
        }
    }
    pub async fn dispatch(&mut self, id: u32) -> Result<()> {
        match self {
            Self::Core(i) => i.dispatch(id).await,
            Self::Component(i) => i.dispatch(id).await,
        }
    }
    pub async fn checkpoint(&mut self) -> Result<Vec<u8>> {
        match self {
            Self::Core(i) => i.checkpoint().await,
            Self::Component(i) => i.checkpoint().await,
        }
    }
    pub async fn restore_checkpoint(&mut self, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Core(i) => i.restore_checkpoint(bytes).await,
            Self::Component(i) => i.restore_checkpoint(bytes).await,
        }
    }
    pub async fn activate(&mut self) -> Result<()> {
        match self {
            Self::Core(i) => i.activate().await,
            Self::Component(i) => i.activate().await,
        }
    }
    pub async fn abort_migration(&mut self) -> Result<()> {
        match self {
            Self::Core(i) => i.abort_migration().await,
            Self::Component(i) => i.abort_migration().await,
        }
    }
}
