//! Admit explicitly delegated command stdio into the instance resource table.
use bexos_userspace::{Memory, Startup};
use bexos_wasm_runtime::{
    context::Context,
    resources::{Kind, READ, TRANSFER, WRITE},
};
use wasmtime::{Result, bail};
pub fn install(context: &mut Context, startup: &Startup) -> Result<()> {
    let Some(options) = bexos_userspace::command::from_startup(startup)
        .map_err(|e| wasmtime::format_err!("command startup: {e:?}"))?
    else {
        return Ok(());
    };
    context.options.arguments = options.arguments;
    context.options.environment = options
        .environment
        .into_iter()
        .map(|(name, value)| bexos_wasm_abi::Environment { name, value })
        .collect();
    for (i, name) in ["wasi:stdin", "wasi:stdout", "wasi:stderr"]
        .into_iter()
        .enumerate()
    {
        let raw = startup.resources[i];
        let (kind, rights) =
            Memory::object_info(raw).map_err(|e| wasmtime::format_err!("stdio type: {e:?}"))?;
        let required = if i == 0 { READ } else { WRITE };
        let admitted = (if rights & 2 != 0 { READ } else { 0 })
            | (if rights & 4 != 0 { WRITE } else { 0 })
            | (if rights & 1 != 0 { TRANSFER } else { 0 });
        if admitted & required == 0 {
            bail!("stdio rights");
        }
        let kind = match kind {
            kernel_fidl::ObjectType::Socket => Kind::Socket,
            kernel_fidl::ObjectType::Channel => Kind::File,
            _ => bail!("stdio object type"),
        };
        context
            .resources
            .insert(crate::host::entry(name, raw, kind, admitted))?;
    }
    Memory::close(startup.resources[3])
        .map_err(|e| wasmtime::format_err!("command metadata close: {e:?}"))?;
    Ok(())
}
