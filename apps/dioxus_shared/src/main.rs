//! Shared Dioxus UI component package.
//!
//! Application components keep their Rust model, signals, callbacks, and Dioxus
//! virtual DOM. This shared component owns the WASM-side DOM/style/layout/text
//! integration boundary and forwards validated scene batches to the native UI
//! host interface.

wit_bindgen::generate!({
    path: ["external/+_repo_rules+wasmtime_wasi_wit/src/p2/wit", "lib/wasm_runtime/wit"],
    world: "bexos:wasm/dioxus-library",
    pub_export_macro: true,
    generate_all,
});

struct SharedDioxus;

impl exports::bexos::wasm::dioxus::Guest for SharedDioxus {
    fn create_view(
        flatland_resource: u32,
        display_resource: u32,
        width: u32,
        height: u32,
    ) -> Result<u32, String> {
        bexos::wasm::ui::create_view(flatland_resource, display_resource, width, height)
    }

    fn configure_view(view: u32, width: u32, height: u32, scale: f32) -> Result<(), String> {
        bexos::wasm::ui::configure_view(view, width, height, scale)
    }

    fn register_asset(view: u32, asset: u32, kind: u32, bytes: Vec<u8>) -> Result<(), String> {
        bexos::wasm::ui::register_asset(view, asset, kind, &bytes)
    }

    fn release_asset(view: u32, asset: u32) -> Result<(), String> {
        bexos::wasm::ui::release_asset(view, asset)
    }

    fn set_node_scene(view: u32, node: u64, batch: Vec<u8>) -> Result<(), String> {
        bexos::wasm::ui::set_node_scene(view, node, &batch)
    }
    fn submit_scene(view: u32, batch: Vec<u8>) -> Result<(), String> {
        bexos::wasm::ui::submit_scene(view, &batch)
    }

    fn submit_document(view: u32, document: Vec<u8>) -> Result<(), String> {
        let document = bexos_dioxus_dom::Document::decode(&document)
            .map_err(|error| format!("document validation: {error:?}"))?;
        bexos::wasm::ui::configure_view(view, document.width, document.height, document.scale)?;
        let bytes = document
            .encode()
            .map_err(|error| format!("document encoding: {error:?}"))?;
        bexos::wasm::ui::submit_document(view, &bytes)
    }

    fn poll_input(view: u32) -> Result<Vec<exports::bexos::wasm::dioxus::InputEvent>, String> {
        Ok(bexos::wasm::ui::poll_input(view)?
            .into_iter()
            .map(|event| exports::bexos::wasm::dioxus::InputEvent {
                kind: event.kind,
                device: event.device,
                id: event.id,
                phase: event.phase,
                x: event.x,
                y: event.y,
                buttons: event.buttons,
                scroll_x: event.scroll_x,
                scroll_y: event.scroll_y,
                code: event.code,
                key_state: event.key_state,
                modifiers: event.modifiers,
                unicode: event.unicode,
            })
            .collect())
    }

    fn get_presentation_status(
        view: u32,
    ) -> Result<exports::bexos::wasm::dioxus::PresentationStatus, String> {
        let status = bexos::wasm::ui::get_presentation_status(view)?;
        Ok(exports::bexos::wasm::dioxus::PresentationStatus {
            accepted_sequence: status.accepted_sequence,
            pending_count: status.pending_count,
            latched_time_ticks: status.latched_time_ticks,
            scene_generation: status.scene_generation,
        })
    }

    fn get_active_backend(
        view: u32,
    ) -> Result<exports::bexos::wasm::dioxus::BackendStatus, String> {
        let status = bexos::wasm::ui::get_active_backend(view)?;
        Ok(exports::bexos::wasm::dioxus::BackendStatus {
            backend: status.backend,
            failure: status.failure,
        })
    }

    fn close_view(view: u32) -> Result<(), String> {
        bexos::wasm::ui::close_view(view)
    }
}

fn main() {}

export!(SharedDioxus);
