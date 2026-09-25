use std::{env, fs};

use bexos_assembly::{
    CompileConfigInput, ProductInput, compile_config_blob, compile_config_blob_from_product,
    generate_config_rust,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("bexos_assembly: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("product") => product(&args[1..]),
        Some("config") => config(&args[1..]),
        Some("config-policy") => {
            let manifest =
                fs::read(required(&args, "--manifest-bin")?).map_err(|e| e.to_string())?;
            let product = fs::read(required(&args, "--product-bin")?).map_err(|e| e.to_string())?;
            let blob = bexos_assembly::compile_config_policy(
                &manifest,
                &product,
                &required(&args, "--package-id")?,
            )?;
            fs::write(required(&args, "--out")?, blob).map_err(|e| e.to_string())
        }
        Some("config-rust") => config_rust(&args[1..]),
        Some("system-image") => system_image(&args[1..]),
        _ => Err("usage: bexos_assembly product|config|config-rust ...".to_string()),
    }
}

fn product(args: &[String]) -> Result<(), String> {
    let product = fs::read(required(args, "--product-bin")?)
        .map_err(|error| format!("read product: {error}"))?;
    let mut bundles = Vec::new();
    for path in repeated(args, "--bundle-bin") {
        bundles.push(fs::read(&path).map_err(|error| format!("read bundle {path}: {error}"))?);
    }
    let mut manifests = Vec::new();
    for spec in repeated(args, "--manifest-bin") {
        let (label, path) = spec
            .split_once('=')
            .ok_or_else(|| "--manifest-bin requires label=path".to_string())?;
        manifests.push((
            label.to_string(),
            fs::read(path).map_err(|error| format!("read manifest {path}: {error}"))?,
        ));
    }
    let mut prebuilt_components = Vec::new();
    for spec in repeated(args, "--prebuilt-manifest-bin") {
        let (label, path) = spec
            .split_once('=')
            .ok_or_else(|| "--prebuilt-manifest-bin requires label=path".to_string())?;
        prebuilt_components.push(bexos_assembly::PrebuiltComponent {
            label: label.to_string(),
            manifest: fs::read(path).map_err(|error| format!("read manifest {path}: {error}"))?,
            placement: bexos_assembly::Placement::SystemImage,
            signer_id: String::new(),
            component_type: "application".into(),
        });
    }
    for spec in repeated(args, "--prebuilt-component") {
        let fields = spec.splitn(5, '|').collect::<Vec<_>>();
        if fields.len() != 5 {
            return Err("--prebuilt-component requires label|placement|type|signer|path".into());
        }
        let placement = match fields[1] {
            "BOOTFS" => bexos_assembly::Placement::Bootfs,
            "SYSTEM_IMAGE" => bexos_assembly::Placement::SystemImage,
            value => return Err(format!("unsupported prebuilt placement {value}")),
        };
        prebuilt_components.push(bexos_assembly::PrebuiltComponent {
            label: fields[0].to_string(),
            manifest: fs::read(fields[4])
                .map_err(|error| format!("read manifest {}: {error}", fields[4]))?,
            placement,
            signer_id: fields[3].to_string(),
            component_type: fields[2].to_string(),
        });
    }
    let architecture = match required(args, "--architecture")?.as_str() {
        "aarch64" => bexos_app_manifest::Architecture::Aarch64,
        "x86_64" => bexos_app_manifest::Architecture::X86_64,
        value => return Err(format!("unsupported architecture {value}")),
    };
    let manifest = bexos_assembly::validate_product_with_components_for(
        ProductInput {
            product: &product,
            bundles: &bundles,
            manifests: &manifests,
        },
        &prebuilt_components,
        architecture,
    )?;
    let index = required(args, "--out-index")?;
    fs::write(index, manifest.index_text()).map_err(|error| format!("write index: {error}"))?;
    if let Some(path) = optional(args, "--out-bootfs-labels") {
        fs::write(path, manifest.labels_for_placement_text("BOOTFS"))
            .map_err(|error| format!("write bootfs labels: {error}"))?;
    }
    if let Some(path) = optional(args, "--out-system-image-labels") {
        fs::write(path, manifest.labels_for_placement_text("SYSTEM_IMAGE"))
            .map_err(|error| format!("write system labels: {error}"))?;
    }
    Ok(())
}

fn system_image(args: &[String]) -> Result<(), String> {
    let base_path = required(args, "--base")?;
    let base = fs::read(&base_path).map_err(|error| format!("read {base_path}: {error}"))?;
    let mut packages = Vec::new();
    for spec in repeated(args, "--package") {
        let (package_id, autoinstall) = spec
            .split_once('=')
            .ok_or_else(|| "--package requires package_id=true|false".to_string())?;
        packages.push((
            package_id.to_string(),
            autoinstall
                .parse::<bool>()
                .map_err(|_| format!("invalid autoinstall value {autoinstall}"))?,
        ));
    }
    let output = bexos_assembly::append_system_image_packages(&base, &packages)?;
    fs::write(required(args, "--out")?, output).map_err(|error| error.to_string())
}

fn config(args: &[String]) -> Result<(), String> {
    let manifest = fs::read(required(args, "--manifest-bin")?)
        .map_err(|error| format!("read manifest: {error}"))?;
    let override_bytes = optional(args, "--override-bin")
        .map(|path| fs::read(&path).map_err(|error| format!("read override {path}: {error}")))
        .transpose()?;
    let product_bytes = optional(args, "--product-bin")
        .map(|path| fs::read(&path).map_err(|error| format!("read product {path}: {error}")))
        .transpose()?;
    let blob = if let Some(product_bytes) = product_bytes {
        compile_config_blob_from_product(
            &manifest,
            &product_bytes,
            &required(args, "--package-id")?,
        )?
    } else {
        compile_config_blob(CompileConfigInput {
            manifest: &manifest,
            override_bytes: override_bytes.as_deref(),
        })?
    };
    fs::write(required(args, "--out")?, blob).map_err(|error| format!("write config: {error}"))
}

fn config_rust(args: &[String]) -> Result<(), String> {
    let manifest = fs::read(required(args, "--manifest-bin")?)
        .map_err(|error| format!("read manifest: {error}"))?;
    let rust = generate_config_rust(&manifest)?;
    fs::write(required(args, "--out")?, rust).map_err(|error| format!("write config rust: {error}"))
}

fn required(args: &[String], name: &str) -> Result<String, String> {
    optional(args, name).ok_or_else(|| format!("missing {name}"))
}

fn optional(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == name)
        .map(|window| window[1].clone())
}

fn repeated(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|window| window[0] == name)
        .map(|window| window[1].clone())
        .collect()
}
