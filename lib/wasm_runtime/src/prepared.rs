//! Immutable, process-local compilation results prepared before quiescence.
//! These objects are never decoded from guest or migration bytes.
use crate::{
    context::Context, instance::CoreInstance, service_component::ServiceInstance,
    service_guest::ServiceGuest,
};
use std::sync::Arc;
use wasmtime::{Engine, Result};
#[derive(Clone)]
enum Code {
    Core(wasmtime::Module),
    Component(wasmtime::component::Component),
}
#[derive(Clone)]
pub struct PreparedService {
    bytes: Arc<[u8]>,
    code: Code,
}
impl PreparedService {
    pub fn compile(engine: &Engine, bytes: Arc<[u8]>, maximum: u64) -> Result<Self> {
        let code = if bytes.get(4..8) == Some(&[1, 0, 0, 0]) {
            Code::Core(crate::instance::compile_module(engine, &bytes, maximum)?)
        } else {
            Code::Component(crate::instance::compile_component(engine, &bytes, maximum)?)
        };
        Ok(Self { bytes, code })
    }
    pub fn matches(&self, bytes: &[u8]) -> bool {
        &*self.bytes == bytes
    }
    pub async fn instantiate(&self, context: Context) -> Result<ServiceGuest> {
        match &self.code {
            Code::Core(module) => {
                let mut instance =
                    CoreInstance::instantiate_compiled(module, self.bytes.clone(), context).await?;
                instance.service_version().await?;
                Ok(ServiceGuest::Core(instance))
            }
            Code::Component(component) => Ok(ServiceGuest::Component(
                ServiceInstance::instantiate_compiled(component, self.bytes.clone(), context)
                    .await?,
            )),
        }
    }
}
