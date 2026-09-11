//! Embedded native views, with all shell authority checked by scened.
use crate::{View, rpc};
use graphics_fidl::*;
#[derive(Clone, Debug)]
pub struct ChildView {
    pub token: u32,
    pub id: u64,
    pub package: String,
    pub width: u32,
    pub height: u32,
    pub focused: bool,
}
fn call(ordinal: u64, request: &impl FidlEncode) -> Result<rpc::Message, String> {
    let mut bytes = vec![0; 4096];
    let mut hs = [HandleRef { raw: 0 }; 16];
    let e = request
        .encode(&mut bytes, &mut hs)
        .map_err(|_| "View encoding failed")?;
    rpc::exchange(
        rpc::find(crate::FLATLAND_SERVICE)?,
        ordinal,
        &bytes[..e.bytes],
        hs[..e.handles].iter().map(|h| h.raw as u32).collect(),
    )
}
fn mutate(ordinal: u64, q: &impl FidlEncode) -> Result<(), String> {
    let m = call(ordinal, q)?;
    let r = FlatlandSessionPresentResponse::decode(&m.data, &[])
        .map_err(|_| "View response invalid")?;
    if r.status == Status::Ok {
        Ok(())
    } else {
        Err(format!("View: {:?}", r.status))
    }
}
impl View {
    pub fn root_node(&self) -> u64 {
        0xd100_0000 | u64::from(self.id())
    }
    pub fn shell_views(&self) -> Result<(u32, u32, Vec<ChildView>), String> {
        let m = call(25, &FlatlandSessionGetShellViewsRequest {})?;
        let hs = m
            .resources
            .iter()
            .map(|h| HandleRef { raw: *h as u64 })
            .collect::<Vec<_>>();
        let r = FlatlandSessionGetShellViewsResponse::decode(&m.data, &hs)
            .map_err(|_| "Invalid shell views")?;
        if r.status != Status::Ok {
            for h in m.resources {
                rpc::close(h);
            }
            return Err("Shell view access denied".into());
        }
        let mut views = Vec::new();
        for i in 0..r.views.len() {
            let v = r.views.get(i).map_err(|_| "Invalid child view")?;
            views.push(ChildView {
                token: v.token.raw as u32,
                id: v.view_id,
                package: v.package_id.into(),
                width: v.width,
                height: v.height,
                focused: v.focused,
            });
        }
        Ok((r.width, r.height, views))
    }
    pub fn embed(
        &self,
        node: u64,
        child: &ChildView,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        fresh: bool,
    ) -> Result<(), String> {
        self.embed_at(self.root_node(), node, child, x, y, width, height, fresh)
    }
    pub fn create_node(&self, node: u64, parent: u64) -> Result<(), String> {
        mutate(1, &FlatlandSessionCreateTransformRequest { node_id: node })?;
        mutate(
            3,
            &FlatlandSessionAddChildRequest {
                parent,
                child: node,
            },
        )
    }
    pub fn translate_node(&self, node: u64, x: i32, y: i32) -> Result<(), String> {
        mutate(
            4,
            &FlatlandSessionSetTranslationRequest {
                node_id: node,
                x,
                y,
            },
        )
    }
    pub fn embed_at(
        &self,
        parent: u64,
        node: u64,
        child: &ChildView,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        fresh: bool,
    ) -> Result<(), String> {
        if fresh {
            mutate(1, &FlatlandSessionCreateTransformRequest { node_id: node })?;
            mutate(
                3,
                &FlatlandSessionAddChildRequest {
                    parent,
                    child: node,
                },
            )?;
        }
        mutate(
            4,
            &FlatlandSessionSetTranslationRequest {
                node_id: node,
                x,
                y,
            },
        )?;
        mutate(
            5,
            &FlatlandSessionSetScaleRequest {
                node_id: node,
                x: width as f32 / child.width.max(1) as f32,
                y: height as f32 / child.height.max(1) as f32,
            },
        )?;
        mutate(
            6,
            &FlatlandSessionSetClipBoundsRequest {
                node_id: node,
                width: child.width,
                height: child.height,
            },
        )?;
        let token = rpc::clone_resource(child.token)?;
        mutate(
            18,
            &FlatlandSessionSetViewContentRequest {
                node_id: node,
                token: HandleRef { raw: token as u64 },
            },
        )
    }
    pub fn destroy_node(&self, parent: u64, node: u64) -> Result<(), String> {
        mutate(
            11,
            &FlatlandSessionRemoveChildRequest {
                parent,
                child: node,
            },
        )?;
        mutate(
            10,
            &FlatlandSessionReleaseTransformRequest { node_id: node },
        )
    }
    pub fn remove_child(&self, node: u64) -> Result<(), String> {
        mutate(
            11,
            &FlatlandSessionRemoveChildRequest {
                parent: self.root_node(),
                child: node,
            },
        )?;
        mutate(
            10,
            &FlatlandSessionReleaseTransformRequest { node_id: node },
        )
    }
    pub fn order_child(&self, node: u64, index: u32) -> Result<(), String> {
        mutate(
            13,
            &FlatlandSessionSetChildOrderRequest {
                parent: self.root_node(),
                child: node,
                index,
            },
        )
    }
    pub fn focus_child(&self, child: &ChildView) -> Result<(), String> {
        let token = rpc::clone_resource(child.token)?;
        mutate(
            26,
            &FlatlandSessionFocusShellViewRequest {
                token: HandleRef { raw: token as u64 },
            },
        )
    }
}
