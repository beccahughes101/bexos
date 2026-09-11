//! Compose an application with the stable Dioxus component at build time.
use std::{fs, path::PathBuf};
use wac_graph::{CompositionGraph, EncodeOptions, types::Package};

const INSTANCE: &str = "bexos:wasm/dioxus@1.0.0";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: compose_dioxus APP_WASM DIOXUS_WASM OUTPUT_WASM".into());
    }
    let app = fs::read(&args[0])?;
    let dependency = fs::read(&args[1])?;
    let mut graph = CompositionGraph::new();
    let app_package = Package::from_bytes("com:bexos/app", None, app, graph.types_mut())
        .map_err(|error| format!("application component package: {error:?}"))?;
    let exports = graph.types()[app_package.ty()]
        .exports
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let app_package = graph
        .register_package(app_package)
        .map_err(|error| format!("application graph package: {error:?}"))?;
    let app_node = graph.instantiate(app_package);
    graph.set_node_name(app_node, "application");

    let dependency = Package::from_bytes(INSTANCE, None, dependency, graph.types_mut())
        .map_err(|error| format!("Dioxus component package: {error:?}"))?;
    let dependency = graph
        .register_package(dependency)
        .map_err(|error| format!("Dioxus graph package: {error:?}"))?;
    let dependency = graph.instantiate(dependency);
    graph.set_node_name(dependency, INSTANCE);
    let export = graph
        .alias_instance_export(dependency, INSTANCE)
        .map_err(|error| format!("Dioxus component export: {error:?}"))?;
    graph
        .set_instantiation_argument(app_node, INSTANCE, export)
        .map_err(|error| format!("Dioxus application import: {error:?}"))?;
    for name in exports {
        let export = graph
            .alias_instance_export(app_node, &name)
            .map_err(|error| format!("application export {name}: {error:?}"))?;
        graph
            .export(export, name)
            .map_err(|error| format!("application graph export: {error:?}"))?;
    }
    fs::write(
        &args[2],
        graph
            .encode(EncodeOptions::default())
            .map_err(|error| format!("component graph encode: {error:?}"))?,
    )?;
    Ok(())
}
