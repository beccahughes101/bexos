//! Reusable Flatland protocol client. Service discovery and capability grants
//! remain the application's responsibility; transactions use stable ordinals.
use crate::{rpc::Rpc, wire};
use bexos_graphics::{Surface, effects::Effects, layout::Properties};
use bexos_userspace::{Channel, Memory};
use graphics_fidl::*;
/// Includes the kernel envelope and all eight admitted input events.
pub const INPUT_RESPONSE_BYTES: usize = 2048;

pub struct Session {
    pub channel: Channel,
    rpc: Rpc,
}
impl Session {
    pub fn new(channel: Channel) -> Self {
        Self {
            channel,
            rpc: Rpc::default(),
        }
    }
    fn mutate<Q: FidlEncode>(&mut self, ordinal: u64, request: &Q) -> Result<(), Status> {
        let r: FlatlandSessionPresentResponse =
            self.rpc.call(&mut self.channel, ordinal, request)?;
        if r.status == Status::Ok {
            Ok(())
        } else {
            Err(r.status)
        }
    }
    pub fn create(&mut self, node_id: u64) -> Result<(), Status> {
        self.mutate(1, &FlatlandSessionCreateTransformRequest { node_id })
    }
    pub fn root(&mut self, node_id: u64) -> Result<(), Status> {
        self.mutate(2, &FlatlandSessionSetRootRequest { node_id })
    }
    pub fn attach(&mut self, parent: u64, child: u64) -> Result<(), Status> {
        self.mutate(3, &FlatlandSessionAddChildRequest { parent, child })
    }
    pub fn translation(&mut self, node_id: u64, x: i32, y: i32) -> Result<(), Status> {
        self.mutate(4, &FlatlandSessionSetTranslationRequest { node_id, x, y })
    }
    pub fn scale(&mut self, node_id: u64, x: f32, y: f32) -> Result<(), Status> {
        self.mutate(5, &FlatlandSessionSetScaleRequest { node_id, x, y })
    }
    pub fn clip(&mut self, node_id: u64, width: u32, height: u32) -> Result<(), Status> {
        self.mutate(
            6,
            &FlatlandSessionSetClipBoundsRequest {
                node_id,
                width,
                height,
            },
        )
    }
    pub fn opacity(&mut self, node_id: u64, opacity: f32) -> Result<(), Status> {
        self.mutate(7, &FlatlandSessionSetOpacityRequest { node_id, opacity })
    }
    pub fn content(&mut self, node_id: u64, buffer: u64, surface: Surface) -> Result<(), Status> {
        let duplicate =
            Memory::duplicate(buffer, 1 | 2 | 16 | 32).map_err(|_| Status::ErrAccessDenied)?;
        let r: FlatlandSessionSetContentResponse = crate::call_owned(
            &mut self.channel,
            8,
            &FlatlandSessionSetContentRequest {
                node_id,
                buffer: HandleRef { raw: duplicate },
                surface: wire(surface),
            },
            &[duplicate],
        )?;
        if r.status == Status::Ok {
            Ok(())
        } else {
            Err(r.status)
        }
    }
    pub fn present(&mut self, presentation_time_ticks: u64) -> Result<(), Status> {
        self.mutate(
            9,
            &FlatlandSessionPresentRequest {
                presentation_time_ticks,
            },
        )
    }
    pub fn remove(&mut self, node_id: u64) -> Result<(), Status> {
        self.mutate(10, &FlatlandSessionReleaseTransformRequest { node_id })
    }
    pub fn detach(&mut self, parent: u64, child: u64) -> Result<(), Status> {
        self.mutate(11, &FlatlandSessionRemoveChildRequest { parent, child })
    }
    pub fn clear(&mut self, node_id: u64) -> Result<(), Status> {
        self.mutate(12, &FlatlandSessionClearContentRequest { node_id })
    }
    pub fn reorder(&mut self, parent: u64, child: u64, index: u32) -> Result<(), Status> {
        self.mutate(
            13,
            &FlatlandSessionSetChildOrderRequest {
                parent,
                child,
                index,
            },
        )
    }
    pub fn presentation_info(
        &mut self,
    ) -> Result<FlatlandSessionGetPresentationInfoResponse, Status> {
        self.rpc.call(
            &mut self.channel,
            14,
            &FlatlandSessionGetPresentationInfoRequest {},
        )
    }
    pub fn read_input(&mut self) -> Result<Vec<NormalizedInputEvent>, Status> {
        let mut request = [0; 64];
        let size = FlatlandSessionReadInputRequest {}
            .encode(&mut request[8..], &mut [])
            .map_err(|_| Status::ErrInvalidArgs)?;
        request[..8].copy_from_slice(&15u64.to_le_bytes());
        self.channel
            .send(&request[..8 + size.bytes], &[])
            .map_err(|_| Status::ErrIo)?;
        // ReadMessage is nonblocking. Keep this exchange outstanding until its
        // reply arrives; otherwise the next Flatland operation consumes input's
        // response and every subsequent response is shifted by one request.
        let mut reply = [0; INPUT_RESPONSE_BYTES];
        let deadline = crate::now_us().saturating_add(60_000_000);
        loop {
            match crate::stream::read_no_handles(self.channel, &mut reply) {
                Ok(bytes) => {
                    let result = (|| {
                        let response = FlatlandSessionReadInputResponse::decode(bytes, &[])
                            .map_err(|_| Status::ErrInvalidArgs)?;
                        if response.status != Status::Ok {
                            return Err(response.status);
                        }
                        (0..response.events.len())
                            .map(|i| response.events.get(i).map_err(|_| Status::ErrInvalidArgs))
                            .collect()
                    })();
                    return result;
                }
                Err(kernel_fidl::Status::ErrTimedOut) if crate::now_us() < deadline => {
                    crate::wait(&[self.channel], deadline);
                }
                Err(_) => {
                    // Responses have no transaction IDs. A timed-out exchange
                    // cannot safely share this endpoint with a later request.
                    crate::close(&[self.channel.0]);
                    self.channel.0 = 0;
                    return Err(Status::ErrIo);
                }
            }
        }
    }
    pub fn present_with_fences(
        &mut self,
        presentation_time_ticks: u64,
        acquire: u64,
        release: u64,
    ) -> Result<u64, Status> {
        let a = Memory::duplicate(acquire, 1 | 2 | 4 | 32).map_err(|_| Status::ErrAccessDenied)?;
        let b = match Memory::duplicate(release, 1 | 2 | 4 | 32) {
            Ok(v) => v,
            Err(_) => {
                let _ = Memory::close(a);
                return Err(Status::ErrAccessDenied);
            }
        };
        let r: FlatlandSessionPresentWithFencesResponse = crate::call_owned(
            &mut self.channel,
            16,
            &FlatlandSessionPresentWithFencesRequest {
                presentation_time_ticks,
                acquire: HandleRef { raw: a },
                release: HandleRef { raw: b },
            },
            &[a, b],
        )?;
        if r.status == Status::Ok {
            Ok(r.sequence)
        } else {
            Err(r.status)
        }
    }
    pub fn view_reference(&mut self) -> Result<ViewReference, Status> {
        let r: FlatlandSessionGetViewReferenceResponse = crate::call(
            &mut self.channel,
            17,
            &FlatlandSessionGetViewReferenceRequest {},
        )?;
        if r.status != Status::Ok {
            if let Some(view) = r.view {
                crate::close(&[view.token.raw]);
            }
            return Err(r.status);
        }
        r.view.ok_or(Status::ErrInvalidArgs)
    }
    pub fn embed(&mut self, node_id: u64, token: u64) -> Result<(), Status> {
        let token = Memory::duplicate(token, 1 | 32).map_err(|_| Status::ErrAccessDenied)?;
        let r: FlatlandSessionSetViewContentResponse = crate::call_owned(
            &mut self.channel,
            18,
            &FlatlandSessionSetViewContentRequest {
                node_id,
                token: HandleRef { raw: token },
            },
            &[token],
        )?;
        if r.status == Status::Ok {
            Ok(())
        } else {
            Err(r.status)
        }
    }
    pub fn clear_view(&mut self, node_id: u64) -> Result<(), Status> {
        self.mutate(19, &FlatlandSessionClearViewContentRequest { node_id })
    }
    pub fn feedback(&mut self) -> Result<FlatlandSessionGetPresentationFeedbackResponse, Status> {
        self.rpc.call(
            &mut self.channel,
            20,
            &FlatlandSessionGetPresentationFeedbackRequest {},
        )
    }
    pub fn effects(&mut self, node_id: u64, effects: Effects) -> Result<(), Status> {
        effects.validate().map_err(|_| Status::ErrInvalidArgs)?;
        self.mutate(
            21,
            &FlatlandSessionSetContentEffectsRequest {
                node_id,
                corner_radius: effects.corner_radius,
                backdrop_radius: effects.backdrop_radius,
            },
        )
    }
    pub fn layout(&mut self, node_id: u64, p: Properties) -> Result<(), Status> {
        p.validate().map_err(|_| Status::ErrInvalidArgs)?;
        self.mutate(
            22,
            &FlatlandSessionSetLayoutRequest {
                node_id,
                mode: p.mode,
                width: p.width,
                height: p.height,
                grow: p.grow,
                gap: p.gap,
                padding: p.padding,
                columns: p.columns,
            },
        )
    }
    pub fn style(
        &mut self,
        node_id: u64,
        p: &bexos_graphics::style::Properties,
    ) -> Result<(), Status> {
        p.validate().map_err(|_| Status::ErrInvalidArgs)?;
        self.mutate(
            23,
            &FlatlandSessionSetStyleRequest {
                node_id,
                identifier: &p.identifier,
                classes: &p.classes,
                inline_style: &p.inline,
            },
        )
    }
    pub fn viewport(&mut self) -> Result<ViewGeometry, Status> {
        let response: FlatlandSessionGetViewportResponse =
            self.rpc
                .call(&mut self.channel, 24, &FlatlandSessionGetViewportRequest {})?;
        if response.status == Status::Ok {
            Ok(response.geometry)
        } else {
            Err(response.status)
        }
    }
}
