//! Shared-engine children retain independent stores, memories and resource IDs.
use crate::{
    component::CommandInstance, context::Context, control::Control, instance::CoreInstance,
    resources::Origin,
};
use std::sync::Arc;
use wasmtime::{Engine, Result, Store, bail};
pub enum Child {
    Core(CoreInstance),
    Command(CommandInstance),
    Service(crate::service_guest::ServiceGuest),
}
impl Child {
    pub async fn instantiate(engine: &Engine, bytes: Arc<[u8]>, context: Context) -> Result<Self> {
        if bytes.get(4..8) == Some(&[1, 0, 0, 0]) {
            Ok(Self::Core(
                CoreInstance::instantiate(engine, bytes, context).await?,
            ))
        } else {
            if context.origin == Origin::Unsigned {
                bail!("unsigned components cannot import ambient WASI");
            }
            let component = crate::instance::compile_component(
                engine,
                &bytes,
                context.options.limits.max_module_bytes,
            )?;
            let service = component
                .component_type()
                .exports(engine)
                .any(|(name, _)| name == "bexos:wasm/lifecycle@1.0.0");
            drop(component);
            if service {
                Ok(Self::Service(
                    crate::service_guest::ServiceGuest::instantiate(engine, bytes, context).await?,
                ))
            } else {
                Ok(Self::Command(
                    CommandInstance::instantiate(engine, bytes, context).await?,
                ))
            }
        }
    }
    pub fn store(&self) -> &Store<Context> {
        match self {
            Self::Core(i) => &i.store,
            Self::Command(i) => &i.store,
            Self::Service(i) => i.store(),
        }
    }
    pub fn store_mut(&mut self) -> &mut Store<Context> {
        match self {
            Self::Core(i) => &mut i.store,
            Self::Command(i) => &mut i.store,
            Self::Service(i) => i.store_mut(),
        }
    }
    pub fn control(&self) -> &Arc<Control> {
        match self {
            Self::Core(i) => &i.control,
            Self::Command(i) => &i.control,
            Self::Service(i) => i.control(),
        }
    }
    pub async fn invoke(&mut self, name: &str, input: i32) -> Result<i32> {
        if self.control().status() != 0 {
            bail!("child is not running");
        }
        match self {
            Self::Service(i) => match i {
                crate::service_guest::ServiceGuest::Core(i) => i.invoke(name, input).await,
                crate::service_guest::ServiceGuest::Component(i) => {
                    if name != "dispatch" {
                        bail!("service child accepts dispatch");
                    }
                    i.dispatch(input as u32).await?;
                    Ok(0)
                }
            },
            Self::Core(i) => i.invoke(name, input).await,
            Self::Command(i) => {
                if name != "run" || input != 0 {
                    bail!("command child accepts run with input zero");
                }
                Ok(i.run().await? as i32)
            }
        }
    }
    pub async fn snapshot(&mut self) -> Result<crate::migration::Snapshot> {
        match self {
            Self::Service(i) => i.snapshot().await,
            Self::Core(i) => i.snapshot().await,
            Self::Command(_) => bail!("live command child requires restart; migration rejected"),
        }
    }
}
