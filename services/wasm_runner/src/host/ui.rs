use super::ui_renderer::UiRenderer;
use bexos_graphics_runtime::flatland::Session;
use bexos_userspace::{Channel, Memory};
use bexos_wasm_runtime::{
    host::{UiBackendStatus, UiInputEvent, UiPresentationStatus},
    resources::Handle,
};
use std::{collections::BTreeMap, sync::Arc};
use wasmtime::{Result, bail};

const FLATLAND_PROTOCOL: &str = "bexos.ui.scened.FlatlandSession";
const DISPLAY_PROTOCOL: &str = "bexos.hardware.display.DisplayCoordinator";

pub struct UiState {
    next_view: u32,
    views: BTreeMap<u32, View>,
}

struct Asset {
    kind: u32,
    bytes: Arc<Vec<u8>>,
}

struct View {
    flatland: Arc<dyn Handle>,
    node_id: u64,
    width: u32,
    height: u32,
    scale: f32,
    assets: BTreeMap<u32, Asset>,
    resolved_fonts: BTreeMap<u32, Arc<bexos_font_client::MappedFont>>,
    releases: Vec<(u64, u64)>,
    retained_document: Option<Vec<u8>>,
    theme_generation: u64,
    last_sequence: u64,
    scene_generation: u64,
    renderer: UiRenderer,
}

impl UiState {
    pub fn new() -> Self {
        Self {
            next_view: 1,
            views: BTreeMap::new(),
        }
    }

    pub fn create_view(
        &mut self,
        flatland: &dyn Handle,
        display: Option<&dyn Handle>,
        width: u32,
        height: u32,
    ) -> Result<u32> {
        require_method(flatland, FLATLAND_PROTOCOL, "Public", 1)?;
        require_method(flatland, FLATLAND_PROTOCOL, "Public", 2)?;
        require_method(flatland, FLATLAND_PROTOCOL, "Public", 8)?;
        require_method(flatland, FLATLAND_PROTOCOL, "Public", 14)?;
        require_method(flatland, FLATLAND_PROTOCOL, "Public", 15)?;
        require_method(flatland, FLATLAND_PROTOCOL, "Public", 16)?;
        require_method(flatland, FLATLAND_PROTOCOL, "Public", 20)?;
        let gpu_allowed = display.is_some_and(|display| {
            grant_allows(display, DISPLAY_PROTOCOL, "GpuTransport", 7)
                && grant_allows(display, DISPLAY_PROTOCOL, "GpuTransport", 8)
                && grant_allows(display, DISPLAY_PROTOCOL, "GpuTransport", 9)
                && grant_allows(display, DISPLAY_PROTOCOL, "GpuTransport", 10)
                && grant_allows(display, DISPLAY_PROTOCOL, "GpuTransport", 12)
        });
        if width == 0
            || height == 0
            || width > bexos_dioxus_scene::MAX_DIMENSION
            || height > bexos_dioxus_scene::MAX_DIMENSION
        {
            bail!("invalid view dimensions");
        }
        let id = self.next_view;
        self.next_view = self
            .next_view
            .checked_add(1)
            .ok_or_else(|| wasmtime::format_err!("view id exhausted"))?;
        let node_id = 0xd100_0000u64 | u64::from(id);
        let mut session = Session::new(Channel(flatland.native()));
        session
            .create(node_id)
            .map_err(|status| wasmtime::format_err!("flatland create: {status:?}"))?;
        session
            .root(node_id)
            .map_err(|status| wasmtime::format_err!("flatland root: {status:?}"))?;
        self.views.insert(
            id,
            View {
                flatland: clone_view_handle(flatland),
                node_id,
                width,
                height,
                scale: 1.0,
                assets: BTreeMap::new(),
                resolved_fonts: BTreeMap::new(),
                releases: Vec::new(),
                retained_document: None,
                theme_generation: 0,
                last_sequence: 0,
                scene_generation: 0,
                renderer: UiRenderer::new(display, gpu_allowed),
            },
        );
        Ok(id)
    }

    pub fn configure_view(&mut self, view: u32, width: u32, height: u32, scale: f32) -> Result<()> {
        let view = self.views.get_mut(&view).ok_or_else(missing_view)?;
        if width == 0
            || height == 0
            || width > bexos_dioxus_scene::MAX_DIMENSION
            || height > bexos_dioxus_scene::MAX_DIMENSION
            || !scale.is_finite()
        {
            bail!("invalid view configuration");
        }
        view.width = width;
        view.height = height;
        view.scale = scale.clamp(0.25, 8.0);
        Ok(())
    }

    pub fn register_asset(&mut self, view: u32, asset: u32, kind: u32, bytes: &[u8]) -> Result<()> {
        let view = self.views.get_mut(&view).ok_or_else(missing_view)?;
        if asset == 0
            || view.assets.len() >= bexos_dioxus_scene::MAX_ASSETS
            || bytes.len() > 4 << 20
        {
            bail!("invalid asset");
        }
        view.assets.insert(
            asset,
            Asset {
                kind,
                bytes: Arc::new(bytes.to_vec()),
            },
        );
        Ok(())
    }

    pub fn release_asset(&mut self, view: u32, asset: u32) -> Result<()> {
        let view = self.views.get_mut(&view).ok_or_else(missing_view)?;
        view.assets
            .remove(&asset)
            .ok_or_else(|| wasmtime::format_err!("missing asset"))?;
        Ok(())
    }

    pub fn set_node_scene(&mut self, id: u32, node: u64, bytes: &[u8]) -> Result<()> {
        let view = self.views.get_mut(&id).ok_or_else(missing_view)?;
        require_method(&*view.flatland, FLATLAND_PROTOCOL, "Public", 8)?;
        let batch = bexos_dioxus_scene::SceneBatch::decode(bytes)
            .map_err(|e| wasmtime::format_err!("invalid node scene: {e:?}"))?;
        validate_assets(view, &batch)?;
        let fonts = font_resources(view);
        let frame = view.renderer.render(&batch, &fonts)?;
        Session::new(Channel(view.flatland.native()))
            .content(node, frame.handle, frame.surface)
            .map_err(|e| wasmtime::format_err!("node content: {e:?}"))?;
        Ok(())
    }
    pub fn submit_scene(&mut self, view: u32, batch: &[u8]) -> Result<()> {
        let view = self.views.get_mut(&view).ok_or_else(missing_view)?;
        view.retained_document = None;
        submit_batch(view, batch)
    }

    pub fn submit_document(
        &mut self,
        id: u32,
        document: &[u8],
        theme: &bexos_ui_runtime::RuntimeTheme,
        fonts: &mut bexos_ui_runtime::FontSet,
        resolved_fonts: &[(u32, Arc<bexos_font_client::MappedFont>)],
    ) -> Result<()> {
        let view = self.views.get_mut(&id).ok_or_else(missing_view)?;
        view.resolved_fonts.extend(resolved_fonts.iter().cloned());
        let document = bexos_dioxus_dom::Document::decode(document)
            .map_err(|error| wasmtime::format_err!("document validation: {error:?}"))?;
        if document.width != view.width || document.height != view.height {
            bail!("document dimensions do not match view");
        }
        let rendered = bexos_ui_runtime::render_document_with_fonts(&document, theme, fonts)
            .map_err(|error| wasmtime::format_err!("document render: {error:?}"))?;
        validate_assets(view, &rendered.batch)?;
        let bytes = rendered
            .batch
            .encode()
            .map_err(|error| wasmtime::format_err!("document scene encode: {error:?}"))?;
        submit_batch(view, &bytes)?;
        view.retained_document = Some(
            document
                .encode()
                .map_err(|error| wasmtime::format_err!("document retain: {error:?}"))?,
        );
        view.theme_generation = rendered.theme_generation;
        Ok(())
    }

    pub fn redraw_documents(
        &mut self,
        theme: &bexos_ui_runtime::RuntimeTheme,
        fonts: &mut bexos_ui_runtime::FontSet,
        resolved_fonts: &[(u32, Arc<bexos_font_client::MappedFont>)],
    ) -> Result<()> {
        for view in self.views.values_mut() {
            view.resolved_fonts.extend(resolved_fonts.iter().cloned());
            let Some(bytes) = view.retained_document.clone() else {
                continue;
            };
            let document = bexos_dioxus_dom::Document::decode(&bytes)
                .map_err(|error| wasmtime::format_err!("retained document: {error:?}"))?;
            let rendered = bexos_ui_runtime::render_document_with_fonts(&document, theme, fonts)
                .map_err(|error| wasmtime::format_err!("document redraw: {error:?}"))?;
            let scene = rendered
                .batch
                .encode()
                .map_err(|error| wasmtime::format_err!("document scene encode: {error:?}"))?;
            submit_batch(view, &scene)?;
            view.theme_generation = rendered.theme_generation;
        }
        Ok(())
    }

    fn submit_scene_bytes(&mut self, view: u32, batch: &[u8]) -> Result<()> {
        let view = self.views.get_mut(&view).ok_or_else(missing_view)?;
        submit_batch(view, batch)
    }

    pub fn submit_scene_compat(&mut self, view: u32, batch: &[u8]) -> Result<()> {
        self.submit_scene_bytes(view, batch)
    }
}

fn submit_batch(view: &mut View, batch: &[u8]) -> Result<()> {
    retire_releases(view);
    if view.releases.len() >= 2 {
        bail!("presentation queue full");
    }
    let batch = bexos_dioxus_scene::SceneBatch::decode(batch)
        .map_err(|error| wasmtime::format_err!("scene validation: {error:?}"))?;
    if batch.width != view.width || batch.height != view.height {
        bail!("scene dimensions do not match view");
    }
    validate_assets(view, &batch)?;
    let fonts = font_resources(view);
    let frame = view.renderer.render(&batch, &fonts)?;
    let mut session = Session::new(Channel(view.flatland.native()));
    if let Err(status) = session.content(view.node_id, frame.handle, frame.surface) {
        drop(frame);
        bail!("flatland content: {status:?}");
    }
    drop(frame);
    let (signal, acquire) =
        Channel::pair().map_err(|error| wasmtime::format_err!("acquire fence: {error:?}"))?;
    let (release, notify) =
        Channel::pair().map_err(|error| wasmtime::format_err!("release fence: {error:?}"))?;
    let sequence = match session.present_with_fences(0, acquire.0, notify.0) {
        Ok(sequence) => sequence,
        Err(status) => {
            let _ = Memory::close(signal.0);
            let _ = Memory::close(acquire.0);
            let _ = Memory::close(release.0);
            let _ = Memory::close(notify.0);
            bail!("flatland present: {status:?}");
        }
    };
    signal
        .send(&[], &[])
        .map_err(|error| wasmtime::format_err!("signal acquire: {error:?}"))?;
    let _ = Memory::close(signal.0);
    let _ = Memory::close(acquire.0);
    let _ = Memory::close(notify.0);
    view.releases.push((sequence, release.0));
    view.last_sequence = sequence;
    if view.scene_generation == 0 {
        bexos_userspace::log(&format!(
            "wasm_runner: native UI first frame submitted backend={}\n",
            view.renderer.backend()
        ));
    }
    view.scene_generation = view.scene_generation.saturating_add(1);
    Ok(())
}

impl UiState {
    pub fn poll_input(&mut self, view: u32) -> Result<Vec<UiInputEvent>> {
        let view = self.views.get_mut(&view).ok_or_else(missing_view)?;
        let mut session = Session::new(Channel(view.flatland.native()));
        Ok(session
            .read_input()
            .map_err(|status| wasmtime::format_err!("flatland input: {status:?}"))?
            .into_iter()
            .map(|event| UiInputEvent {
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

    pub fn presentation_status(&mut self, view: u32) -> Result<UiPresentationStatus> {
        let view = self.views.get_mut(&view).ok_or_else(missing_view)?;
        retire_releases(view);
        let mut session = Session::new(Channel(view.flatland.native()));
        let info = session
            .presentation_info()
            .map_err(|status| wasmtime::format_err!("flatland presentation: {status:?}"))?;
        Ok(UiPresentationStatus {
            accepted_sequence: info.accepted_sequence,
            pending_count: info.pending_count,
            latched_time_ticks: info.latched_time_ticks,
            scene_generation: view.scene_generation,
        })
    }

    pub fn active_backend(&mut self, view: u32) -> Result<UiBackendStatus> {
        let view = self.views.get(&view).ok_or_else(missing_view)?;
        Ok(UiBackendStatus {
            backend: view.renderer.backend().into(),
            failure: view.renderer.failure(),
        })
    }

    pub fn quiescent(&mut self) -> bool {
        for view in self.views.values_mut() {
            retire_releases(view);
            if !view.renderer.drain_for_migration() {
                return false;
            }
        }
        true
    }

    pub fn retained_documents(&self) -> Vec<Vec<u8>> {
        self.views
            .values()
            .filter_map(|view| view.retained_document.clone())
            .collect()
    }

    pub fn resources(&self) -> Vec<u64> {
        self.views
            .values()
            .flat_map(|v| v.releases.iter().map(|(_, h)| *h))
            .collect()
    }
    pub fn encode(&self) -> Result<Vec<u8>, bexos_migration::Error> {
        use bexos_migration::{Error, codec::Encoder};
        let mut w = Encoder::new();
        w.word(2);
        w.word(self.next_view as u64);
        w.word(self.views.len() as u64);
        let mut assets = 0usize;
        for (id, v) in &self.views {
            for n in [
                *id as u64,
                v.flatland.native(),
                v.node_id,
                v.width as u64,
                v.height as u64,
                v.scale.to_bits() as u64,
                v.theme_generation,
                v.last_sequence,
                v.scene_generation,
            ] {
                w.word(n);
            }
            w.word(v.releases.len() as u64);
            for (sequence, handle) in &v.releases {
                w.word(*sequence);
                w.word(*handle);
            }
            w.word(v.assets.len() as u64);
            for (id, a) in &v.assets {
                assets = assets.checked_add(a.bytes.len()).ok_or(Error::Capacity)?;
                if assets > 4 << 20 {
                    return Err(Error::Capacity);
                }
                w.word(*id as u64);
                w.word(a.kind as u64);
                w.bytes(a.bytes.as_ref());
            }
            match &v.retained_document {
                Some(document) => w.bytes(document),
                None => w.bytes(&[]),
            }
        }
        Ok(w.finish())
    }
    pub fn decode(
        bytes: &[u8],
        resources: &[(u32, bexos_wasm_runtime::resources::Entry)],
    ) -> Result<Self, bexos_migration::Error> {
        use bexos_migration::{Error, codec::Decoder};
        if bytes.is_empty() {
            return Ok(Self::new());
        }
        let mut r = Decoder::new(bytes);
        let version = r.word()?;
        if !(1..=2).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        let next_view: u32 = r.word()?.try_into().map_err(|_| Error::Capacity)?;
        if next_view == 0 {
            return Err(Error::InvalidData);
        }
        let mut asset_bytes = 0usize;
        let mut out = Self {
            next_view,
            views: BTreeMap::new(),
        };
        for _ in 0..r.count(16)? {
            let id: u32 = r.word()?.try_into().map_err(|_| Error::Capacity)?;
            let native = r.word()?;
            let flatland = resources
                .iter()
                .find(|(_, e)| e.handle.native() == native)
                .ok_or(Error::InvalidData)?
                .1
                .handle
                .clone();
            if !grant_allows(&*flatland, FLATLAND_PROTOCOL, "Public", 8) {
                return Err(Error::InvalidData);
            }
            let node_id = r.word()?;
            let width: u32 = r.word()?.try_into().map_err(|_| Error::Capacity)?;
            let height: u32 = r.word()?.try_into().map_err(|_| Error::Capacity)?;
            let scale = f32::from_bits(r.word()?.try_into().map_err(|_| Error::Capacity)?);
            let theme_generation = if version >= 2 { r.word()? } else { 0 };
            let last_sequence = r.word()?;
            let scene_generation = r.word()?;
            if id == 0
                || id >= next_view
                || width == 0
                || height == 0
                || width > 8192
                || height > 8192
                || !scale.is_finite()
                || !(0.25..=8.).contains(&scale)
            {
                return Err(Error::InvalidData);
            }
            let mut releases = Vec::new();
            for _ in 0..r.count(2)? {
                let sequence = r.word()?;
                let handle = r.word()?;
                if handle == 0 {
                    return Err(Error::InvalidData);
                }
                releases.push((sequence, handle));
            }
            let mut assets = BTreeMap::new();
            for _ in 0..r.count(bexos_dioxus_scene::MAX_ASSETS)? {
                let asset = r.word()?.try_into().map_err(|_| Error::Capacity)?;
                let kind = r.word()?.try_into().map_err(|_| Error::Capacity)?;
                let bytes = r.bytes(4 << 20)?.to_vec();
                asset_bytes = asset_bytes
                    .checked_add(bytes.len())
                    .ok_or(Error::Capacity)?;
                if asset_bytes > 4 << 20 {
                    return Err(Error::Capacity);
                }
                if assets
                    .insert(
                        asset,
                        Asset {
                            kind,
                            bytes: Arc::new(bytes),
                        },
                    )
                    .is_some()
                {
                    return Err(Error::InvalidData);
                }
            }
            let retained_document = if version >= 2 {
                let retained = r.bytes(bexos_dioxus_dom::MAX_DOCUMENT_BYTES)?.to_vec();
                if retained.is_empty() {
                    None
                } else {
                    bexos_dioxus_dom::Document::decode(&retained)
                        .map_err(|_| Error::InvalidData)?;
                    Some(retained)
                }
            } else {
                None
            };
            let view = View {
                flatland,
                node_id,
                width,
                height,
                scale,
                assets,
                releases,
                retained_document,
                theme_generation,
                last_sequence,
                scene_generation,
                renderer: UiRenderer::new(None, false),
                resolved_fonts: BTreeMap::new(),
            };
            if out.views.insert(id, view).is_some() {
                return Err(Error::InvalidData);
            }
        }
        r.finish()?;
        Ok(out)
    }
    pub fn close_view(&mut self, view: u32) -> Result<()> {
        let Some(mut view) = self.views.remove(&view) else {
            return Err(missing_view());
        };
        for (_, release) in view.releases.drain(..) {
            let _ = Memory::close(release);
        }
        let mut session = Session::new(Channel(view.flatland.native()));
        session
            .clear(view.node_id)
            .map_err(|status| wasmtime::format_err!("flatland clear: {status:?}"))?;
        session
            .remove(view.node_id)
            .map_err(|status| wasmtime::format_err!("flatland remove: {status:?}"))?;
        Ok(())
    }
}

fn validate_assets(view: &View, batch: &bexos_dioxus_scene::SceneBatch) -> Result<()> {
    for command in &batch.commands {
        match command {
            bexos_dioxus_scene::Command::Image(image) => {
                let asset = view
                    .assets
                    .get(&image.image_asset)
                    .ok_or_else(|| wasmtime::format_err!("missing image asset"))?;
                if asset.kind != 1 || asset.bytes.is_empty() {
                    bail!("invalid image asset");
                }
            }
            bexos_dioxus_scene::Command::Glyphs(run) => {
                let package_asset = view
                    .assets
                    .get(&run.font_asset)
                    .is_some_and(|asset| asset.kind == 2 && !asset.bytes.is_empty());
                if !package_asset && !view.resolved_fonts.contains_key(&run.font_asset) {
                    bail!("missing font asset");
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn font_resources(view: &View) -> BTreeMap<u32, bexos_dioxus_render::FontResource> {
    let mut fonts = view
        .resolved_fonts
        .iter()
        .map(|(asset, font)| {
            let data: Arc<dyn AsRef<[u8]> + Send + Sync> = font.clone();
            (
                *asset,
                bexos_dioxus_render::FontResource {
                    data,
                    collection_index: font.collection_index,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    for (asset, resource) in &view.assets {
        if resource.kind == 2 && !resource.bytes.is_empty() {
            let data: Arc<dyn AsRef<[u8]> + Send + Sync> = resource.bytes.clone();
            fonts.insert(
                *asset,
                bexos_dioxus_render::FontResource {
                    data,
                    collection_index: 0,
                },
            );
        }
    }
    fonts
}

fn retire_releases(view: &mut View) {
    view.releases
        .retain(|(_, handle)| match Channel(*handle).try_recv() {
            Ok(_) | Err(kernel_fidl::Status::ErrPeerClosed) => {
                let _ = Memory::close(*handle);
                false
            }
            Err(_) => true,
        });
}

fn require_method(
    handle: &dyn Handle,
    protocol: &'static str,
    capability: &'static str,
    ordinal: u64,
) -> Result<()> {
    if grant_allows(handle, protocol, capability, ordinal) {
        Ok(())
    } else {
        bail!("missing {protocol}.{capability} method {ordinal}")
    }
}

fn grant_allows(
    handle: &dyn Handle,
    protocol: &'static str,
    capability: &'static str,
    ordinal: u64,
) -> bool {
    handle.grant().is_some_and(|grant| {
        (grant.protocol == protocol
            || (grant.service == protocol
                && grant.protocol == protocol.rsplit('.').next().unwrap_or(protocol)))
            && grant.capability == capability
            && grant.method_ordinals.contains(&ordinal)
    })
}

fn clone_view_handle(handle: &dyn Handle) -> Arc<dyn Handle> {
    Arc::new(super::NativeHandle {
        raw: handle.native(),
        kind: handle.kind(),
        rights: handle.rights(),
        companions: handle.companions().to_vec(),
        allowed_methods: handle.allowed_methods().map(|methods| methods.to_vec()),
        grant: handle.grant().cloned(),
        ownership: Some(Arc::new(std::sync::atomic::AtomicBool::new(false))),
    })
}

fn missing_view() -> wasmtime::Error {
    wasmtime::format_err!("missing view")
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use bexos_wasm_runtime::resources::{Entry, Grant, Kind};
    struct TestHandle(Grant);
    impl Handle for TestHandle {
        fn native(&self) -> u64 {
            55
        }
        fn rights(&self) -> u32 {
            7
        }
        fn kind(&self) -> Kind {
            Kind::Channel
        }
        fn grant(&self) -> Option<&Grant> {
            Some(&self.0)
        }
    }
    #[test]
    fn native_view_adoption_preserves_resources_without_calling_the_compositor() {
        let handle: Arc<dyn Handle> = Arc::new(TestHandle(Grant {
            service: FLATLAND_PROTOCOL.into(),
            protocol: FLATLAND_PROTOCOL.into(),
            capability: "Public".into(),
            method_ordinals: vec![8],
            permission_values: vec![],
            caller_package: Some("shell".into()),
            caller_uid: Some(1000),
            caller_foreground: true,
        }));
        let resources = vec![(
            4,
            Entry {
                name: FLATLAND_PROTOCOL.into(),
                handle: handle.clone(),
            },
        )];
        let mut source = UiState::new();
        source.next_view = 2;
        source.views.insert(
            1,
            View {
                flatland: handle,
                node_id: 0xd1000001,
                width: 800,
                height: 600,
                scale: 1.,
                assets: BTreeMap::new(),
                resolved_fonts: BTreeMap::new(),
                releases: vec![(9, 99)],
                retained_document: Some(
                    bexos_dioxus_dom::Document {
                        version: bexos_dioxus_dom::VERSION,
                        width: 800,
                        height: 600,
                        scale: 1.,
                        clear_rgba: [0, 0, 0, 0],
                        stylesheets: Vec::new(),
                        author_stylesheets: Vec::new(),
                        root: bexos_dioxus_dom::Node::element(1, "main")
                            .style("width:800px;height:600px"),
                    }
                    .encode()
                    .unwrap(),
                ),
                theme_generation: 7,
                last_sequence: 9,
                scene_generation: 3,
                renderer: UiRenderer::new(None, false),
            },
        );
        let bytes = source.encode().unwrap();
        let restored = UiState::decode(&bytes, &resources).unwrap();
        assert_eq!(restored.encode().unwrap(), bytes);
        assert_eq!(restored.resources(), vec![99]);
        assert!(UiState::decode(&bytes, &[]).is_err());
        for n in 1..bytes.len() {
            assert!(UiState::decode(&bytes[..n], &resources).is_err());
        }
        // A rejected candidate has no mutation path into the retained source.
        assert_eq!(source.views[&1].last_sequence, 9);
    }
}
