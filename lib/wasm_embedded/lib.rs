//! Trusted build artifacts, selected only by the digest of raw WASM input.
//! Package and migration payloads can never supply serialized executable code.
use sha2::{Digest, Sha256};
use wasmtime::{Engine, Result, component::Component};
mod packed;

const BRUSH_DIGEST: &[u8; 32] = include_bytes!(env!("BRUSH_PULLEY_DIGEST"));
const BRUSH_CODE: &[u8] = include_bytes!(env!("BRUSH_PULLEY_CODE"));
const SYSUI_DIGEST: &[u8; 32] = include_bytes!(env!("SYSUI_PULLEY_DIGEST"));
const SYSUI_CODE: &[u8] = include_bytes!(env!("SYSUI_PULLEY_CODE"));
const USERUI_DIGEST: &[u8; 32] = include_bytes!(env!("USERUI_PULLEY_DIGEST"));
const USERUI_CODE: &[u8] = include_bytes!(env!("USERUI_PULLEY_CODE"));
const DIOXUS_DEMO_DIGEST: &[u8; 32] = include_bytes!(env!("DIOXUS_DEMO_PULLEY_DIGEST"));
const DIOXUS_DEMO_CODE: &[u8] = include_bytes!(env!("DIOXUS_DEMO_PULLEY_CODE"));

pub fn component(engine: &Engine, bytes: &[u8]) -> Option<Result<Component>> {
    let digest = Sha256::digest(bytes);
    let code = [
        (BRUSH_DIGEST, BRUSH_CODE),
        (SYSUI_DIGEST, SYSUI_CODE),
        (USERUI_DIGEST, USERUI_CODE),
        (DIOXUS_DEMO_DIGEST, DIOXUS_DEMO_CODE),
    ]
    .into_iter()
    .find_map(|(expected, code)| (digest.as_slice() == expected).then_some(code))?;
    // SAFETY: these bytes are linked into the authenticated runtime executable,
    // produced by Bazel with the same Wasmtime version and shared configuration.
    // The input digest selects only that exact component. Wasmtime also checks
    // engine compatibility; no bytes from a package or checkpoint reach this API.
    Some(packed::decode(code).and_then(|code| unsafe { Component::deserialize(engine, &code) }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_brush_matches_raw_input_and_supports_manifest_stack_limits() {
        let bytes = include_bytes!(env!("BRUSH_RAW_WASM"));
        let mut limits = bexos_wasm_abi::Limits::default();
        limits.max_stack_bytes = 1 << 20;
        let engine = wasmtime::Engine::new(&bexos_wasm_engine::config(&limits).unwrap()).unwrap();
        super::component(&engine, bytes).unwrap().unwrap();
        let mut modified = bytes.to_vec();
        modified[8] ^= 1;
        assert!(super::component(&engine, &modified).is_none());
    }

    #[test]
    fn arbitrary_input_cannot_select_trusted_code() {
        let engine = wasmtime::Engine::new(
            &bexos_wasm_engine::config(&bexos_wasm_abi::Limits::default()).unwrap(),
        )
        .unwrap();
        assert!(super::component(&engine, b"\0asm\x0d\0\x01\0").is_none());
        assert!(super::component(&engine, super::BRUSH_CODE).is_none());
    }

    #[test]
    fn composed_graphs_select_their_trusted_pulley_artifacts() {
        let engine = wasmtime::Engine::new(
            &bexos_wasm_engine::config(&bexos_wasm_abi::Limits::default()).unwrap(),
        )
        .unwrap();
        for bytes in [
            include_bytes!(env!("SYSUI_COMPOSED")).as_slice(),
            include_bytes!(env!("USERUI_COMPOSED")).as_slice(),
            include_bytes!(env!("DIOXUS_DEMO_COMPOSED")).as_slice(),
        ] {
            super::component(&engine, bytes).unwrap().unwrap();
        }
    }
}
