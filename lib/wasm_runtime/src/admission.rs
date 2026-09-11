//! One admission policy for both canonical components and core hostcalls.
use crate::{context::Context, resources::Origin};
use bexos_wasm_abi::WasmRunnerOptions;
use wasmtime::{Result, bail};

pub(crate) fn child_context(
    parent: &Context,
    options: WasmRunnerOptions,
    payload: &[u8],
    signature: &[u8],
    grants: &[u32],
    remaining_fuel: u64,
) -> Result<Context> {
    let child = &options.limits;
    let limits = &parent.options.limits;
    options
        .validate()
        .map_err(|e| wasmtime::format_err!("child options: {e:?}"))?;
    if parent.restoring
        || parent.next_child == u32::MAX
        || signature.len() > 32768
        || grants.len() > 16
        || payload.len() as u64 > child.max_module_bytes
        || child.max_module_bytes > limits.max_module_bytes
        || child.max_memory_pages > limits.max_memory_pages
        || child.max_table_elements > limits.max_table_elements
        || child.max_stack_bytes > limits.max_stack_bytes
        || child.max_handles > limits.max_handles
        || child.max_children > limits.max_children
        || child.fuel_slice > limits.fuel_slice
        || child.fuel > remaining_fuel
    {
        bail!("child admission exceeds parent budget");
    }
    let origin = if signature.is_empty() {
        Origin::Unsigned
    } else {
        parent.host.verify_signature(payload, signature)?;
        Origin::Signed
    };
    if origin == Origin::Unsigned && !grants.is_empty() {
        bail!("unsigned capability delegation forbidden");
    }
    let permit = parent.budget.child(limits.max_children)?;
    let mut context = Context::new(options, parent.host.clone(), origin, parent.budget.clone());
    context.child_permit = Some(permit);
    for (index, id) in grants.iter().enumerate() {
        if grants[..index].contains(id) {
            bail!("duplicate child grant");
        }
        context
            .resources
            .insert(parent.resources.transferable(*id)?.clone())?;
    }
    Ok(context)
}
