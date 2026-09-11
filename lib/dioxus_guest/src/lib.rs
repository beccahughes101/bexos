//! Guest-side helpers for Rust Dioxus applications that render through the
//! separately packaged `com.bexos.lib.dioxus` WASM component.

wit_bindgen::generate!({
    path: ["external/+_repo_rules+wasmtime_wasi_wit/src/p2/wit", "lib/wasm_runtime/wit"],
    world: "bexos:wasm/dioxus-app",
    pub_export_macro: true,
    generate_all,
});

pub use bexos_dioxus_dom as dom;
pub use bexos_dioxus_scene as scene;
pub mod rpc;
pub mod views;

pub const SHARED_COMPONENT_PACKAGE: &str = "com.bexos.lib.dioxus";
pub const SHARED_COMPONENT_EXPORT: &str = "bexos:wasm/dioxus@1.0.0";
pub const SHARED_COMPONENT_ABI: u32 = 1;
pub const SHARED_COMPONENT_MOUNT: &str = "/deps/com.bexos.lib.dioxus";
pub const SHARED_COMPONENT_INSTANCE: &str = "bexos:wasm/dioxus@1.0.0";
pub const FLATLAND_SERVICE: &str = "bexos.ui.scened.FlatlandSession";
pub const DISPLAY_SERVICE: &str = "bexos.hardware.display.DisplayCoordinator";

pub struct View {
    id: u32,
}

impl View {
    /// Adopt a logical view ID from this instance's validated lifecycle checkpoint.
    pub fn adopt(id: u32) -> Self {
        Self { id }
    }
    pub fn set_node_scene(&self, node: u64, batch: &scene::SceneBatch) -> Result<(), String> {
        bexos::wasm::dioxus::set_node_scene(
            self.id,
            node,
            &batch.encode().map_err(|e| format!("{e:?}"))?,
        )
    }

    pub fn open(width: u32, height: u32) -> Result<Self, String> {
        let flatland = bexos::wasm::kernel::resource_find(FLATLAND_SERVICE)
            .ok_or_else(|| "missing FlatlandSession grant".to_string())?;
        let display = bexos::wasm::kernel::resource_find(DISPLAY_SERVICE).unwrap_or(0);
        let id = bexos::wasm::dioxus::create_view(flatland, display, width, height)?;
        Ok(Self { id })
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn configure(&self, width: u32, height: u32, scale: f32) -> Result<(), String> {
        bexos::wasm::dioxus::configure_view(self.id, width, height, scale)
    }

    pub fn register_asset(&self, asset: u32, kind: AssetKind, bytes: &[u8]) -> Result<(), String> {
        bexos::wasm::dioxus::register_asset(self.id, asset, kind as u32, bytes)
    }

    pub fn release_asset(&self, asset: u32) -> Result<(), String> {
        bexos::wasm::dioxus::release_asset(self.id, asset)
    }

    pub fn submit(&self, batch: &scene::SceneBatch) -> Result<(), String> {
        bexos::wasm::dioxus::submit_scene(
            self.id,
            &batch.encode().map_err(|error| format!("{error:?}"))?,
        )
    }

    pub fn submit_document(&self, document: &dom::Document) -> Result<(), String> {
        bexos::wasm::dioxus::submit_document(
            self.id,
            &document.encode().map_err(|error| format!("{error:?}"))?,
        )
    }

    pub fn input(&self) -> Result<Vec<InputEvent>, String> {
        Ok(bexos::wasm::dioxus::poll_input(self.id)?
            .into_iter()
            .map(|event| InputEvent {
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

    pub fn presentation(&self) -> Result<PresentationStatus, String> {
        let status = bexos::wasm::dioxus::get_presentation_status(self.id)?;
        Ok(PresentationStatus {
            accepted_sequence: status.accepted_sequence,
            pending_count: status.pending_count,
            latched_time_ticks: status.latched_time_ticks,
            scene_generation: status.scene_generation,
        })
    }

    pub fn viewport(&self) -> Result<Viewport, String> {
        let status = self.presentation()?;
        Ok(Viewport {
            scene_generation: status.scene_generation,
        })
    }

    pub fn backend(&self) -> Result<BackendStatus, String> {
        let status = bexos::wasm::dioxus::get_active_backend(self.id)?;
        Ok(BackendStatus {
            backend: status.backend,
            failure: status.failure,
        })
    }

    pub fn close(self) -> Result<(), String> {
        bexos::wasm::dioxus::close_view(self.id)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AssetKind {
    Image = 1,
    Font = 2,
}

#[derive(Clone, Debug)]
pub struct InputEvent {
    pub kind: u32,
    pub device: u64,
    pub id: u32,
    pub phase: u32,
    pub x: f64,
    pub y: f64,
    pub buttons: u32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub code: u32,
    pub key_state: u32,
    pub modifiers: u32,
    pub unicode: u32,
}

#[derive(Clone, Debug)]
pub struct PresentationStatus {
    pub accepted_sequence: u64,
    pub pending_count: u32,
    pub latched_time_ticks: u64,
    pub scene_generation: u64,
}

#[derive(Clone, Debug)]
pub struct BackendStatus {
    pub backend: String,
    pub failure: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Viewport {
    pub scene_generation: u64,
}
