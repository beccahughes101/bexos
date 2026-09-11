use std::sync::Arc;
use wac_graph::{CompositionGraph, EncodeOptions, types::Package};

pub fn compose_application(
    app: Arc<[u8]>,
    dependencies: &[bexos_wasm_abi::ComponentDependency],
    dependency_bytes: Vec<Arc<[u8]>>,
    maximum: u64,
) -> wasmtime::Result<Arc<[u8]>> {
    bexos_wasm_runtime::instance::validate_bytes(&app, maximum)?;
    if dependencies.is_empty() {
        // Instantiation validates and compiles the payload exactly once. A
        // preflight Component::new here bypasses the embedded Brush artifact
        // and needlessly JIT-compiles it before loading that prepared image.
        // Core modules also belong on the ordinary module instantiation path.
        return Ok(app);
    }
    if dependencies.len() != dependency_bytes.len() || dependencies.len() > 16 {
        wasmtime::bail!("component dependency count mismatch");
    }
    let mut total = app.len() as u64;
    for bytes in &dependency_bytes {
        bexos_wasm_runtime::instance::validate_bytes(bytes, maximum)?;
        total = total
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| wasmtime::format_err!("component graph byte limit"))?;
    }
    if total > maximum.saturating_mul(17) {
        wasmtime::bail!("component graph byte limit");
    }

    let mut graph = CompositionGraph::new();
    let app_package = Package::from_bytes(
        "com:bexos/app",
        None,
        app.as_ref().to_vec(),
        graph.types_mut(),
    )
    .map_err(|error| wasmtime::format_err!("application component package: {error:?}"))?;
    let exports = graph.types()[app_package.ty()]
        .exports
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let app_id = graph
        .register_package(app_package)
        .map_err(|error| wasmtime::format_err!("application graph package: {error:?}"))?;
    let app_node = graph.instantiate(app_id);
    graph.set_node_name(app_node, "application");

    for (dependency, bytes) in dependencies.iter().zip(dependency_bytes.iter()) {
        validate_dependency_identity(dependency)?;
        let package = Package::from_bytes(
            &dependency.import.instance_name,
            None,
            bytes.as_ref().to_vec(),
            graph.types_mut(),
        )
        .map_err(|error| {
            wasmtime::format_err!(
                "component package {}: {error:?}",
                dependency.import.instance_name
            )
        })?;
        let package_id = graph
            .register_package(package)
            .map_err(|error| wasmtime::format_err!("component graph package: {error:?}"))?;
        let instance = graph.instantiate(package_id);
        graph.set_node_name(instance, dependency.import.instance_name.clone());
        let export = graph
            .alias_instance_export(instance, &dependency.import.export_name)
            .map_err(|error| {
                wasmtime::format_err!(
                    "component export {}.{}: {error:?}",
                    dependency.import.instance_name,
                    dependency.import.export_name
                )
            })?;
        graph
            .set_instantiation_argument(app_node, &dependency.import.instance_name, export)
            .map_err(|error| {
                wasmtime::format_err!(
                    "component import {}: {error:?}",
                    dependency.import.instance_name
                )
            })?;
    }

    // Instantiation alone does not export anything from the enclosing graph.
    // Preserve the application's command and lifecycle interfaces, never the
    // dependency's private exports.
    for name in exports {
        let export = graph
            .alias_instance_export(app_node, &name)
            .map_err(|e| wasmtime::format_err!("application export {name}: {e:?}"))?;
        graph
            .export(export, name)
            .map_err(|e| wasmtime::format_err!("application graph export: {e:?}"))?;
    }
    let bytes = graph
        .encode(EncodeOptions::default())
        .map_err(|error| wasmtime::format_err!("component graph encode: {error:?}"))?;
    if bytes.len() as u64 > maximum.saturating_mul(17) {
        wasmtime::bail!("composed component graph byte limit");
    }
    bexos_userspace::log(&format!(
        "wasm_runner: composed component graph dependencies={} bytes={}\n",
        dependencies.len(),
        bytes.len()
    ));
    Ok(Arc::from(bytes))
}

fn validate_dependency_identity(
    dependency: &bexos_wasm_abi::ComponentDependency,
) -> wasmtime::Result<()> {
    if dependency.import.package_name.is_empty()
        || dependency.import.package_name.len() > 128
        || dependency.import.export_name.is_empty()
        || dependency.import.export_name.len() > 128
        || dependency.import.abi_version == 0
        || dependency.import.instance_name.is_empty()
        || dependency.import.instance_name.len() > 128
    {
        wasmtime::bail!("invalid component dependency identity");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bexos_wasm_runtime::{
        budget::Budget,
        context::Context,
        host::Host,
        resources::{Entry, Handle, Origin},
        service_guest::ServiceGuest,
    };
    struct InertHost;
    impl Host for InertHost {
        fn monotonic_ns(&self) -> u64 {
            0
        }
        fn log(&self, _: &[u8]) {}
        fn channel_write(&self, _: &dyn Handle, _: &[u8], _: &[Entry]) -> wasmtime::Result<()> {
            wasmtime::bail!("unavailable")
        }
        fn channel_read(
            &self,
            _: &dyn Handle,
            _: usize,
            _: usize,
        ) -> wasmtime::Result<(Vec<u8>, Vec<Arc<dyn Handle>>)> {
            wasmtime::bail!("unavailable")
        }
    }

    fn read_runfile(path: &str) -> Vec<u8> {
        if let Ok(bytes) = std::fs::read(path) {
            return bytes;
        }
        let srcdir = std::env::var("TEST_SRCDIR").unwrap();
        let workspace = std::env::var("TEST_WORKSPACE").unwrap_or_else(|_| "_main".into());
        std::fs::read(std::path::Path::new(&srcdir).join(workspace).join(path)).unwrap()
    }

    #[test]
    fn standalone_core_module_reaches_its_normal_execution_path() {
        let engine = bexos_wasm_runtime::engine::engine().unwrap();
        let bytes = wat::parse_str("(module (func (export \"_start\")))").unwrap();
        let bytes = compose_application(bytes.into(), &[], vec![], 1 << 20).unwrap();
        let context = Context::new(
            Default::default(),
            Arc::new(InertHost),
            Origin::Signed,
            Budget::new(128 << 20),
        );
        let mut instance = crate::executor::block_on(
            bexos_wasm_runtime::instance::CoreInstance::instantiate(&engine, bytes, context),
        )
        .unwrap();
        crate::executor::block_on(instance.run()).unwrap();
    }

    #[test]
    fn composes_actual_dioxus_demo_with_shared_library() {
        let engine = bexos_wasm_runtime::engine::engine().unwrap();
        let app: Arc<[u8]> = read_runfile(env!("DIOXUS_DEMO_COMPONENT")).into();
        let dependency_bytes: Arc<[u8]> = read_runfile(env!("DIOXUS_SHARED_COMPONENT")).into();
        let dependency = bexos_wasm_abi::ComponentDependency {
            import: bexos_wasm_abi::ComponentImport {
                package_name: "com.bexos.lib.dioxus".into(),
                export_name: "bexos:wasm/dioxus@1.0.0".into(),
                abi_version: 1,
                instance_name: "bexos:wasm/dioxus@1.0.0".into(),
            },
            module_len: dependency_bytes.len() as u64,
        };
        let composed =
            compose_application(app, &[dependency], vec![dependency_bytes], 64 << 20).unwrap();
        assert!(
            bexos_wasm_embedded::component(&engine, &composed)
                .unwrap()
                .is_ok(),
            "runtime composition must match the digest-selected build artifact"
        );
        let component = wasmtime::component::Component::new(&engine, &composed).unwrap();
        assert!(
            component
                .component_type()
                .get_export(&engine, "bexos:wasm/lifecycle@1.0.0")
                .is_some()
        );
        assert!(
            component
                .component_type()
                .get_export(&engine, "bexos:wasm/dioxus@1.0.0")
                .is_none()
        );
        let context = Context::new(
            Default::default(),
            Arc::new(InertHost),
            Origin::Signed,
            Budget::new(128 << 20),
        );
        let mut service =
            crate::executor::block_on(ServiceGuest::instantiate(&engine, composed, context))
                .unwrap();
        assert!(
            !crate::executor::block_on(service.checkpoint())
                .unwrap()
                .is_empty()
        );
    }
}
