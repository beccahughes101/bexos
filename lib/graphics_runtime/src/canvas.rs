use crate::{Mapping, call, surface, wire};
use bexos_graphics::Surface;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, Memory, live_migration::Resource};
use graphics_fidl::*;
pub struct Canvas {
    pub display: Channel,
    pub surface: Surface,
    pub output: Mapping,
    pub generation: u64,
    pub client_id: u64,
}
impl Canvas {
    pub fn connect(mut display: Channel, acquire: bool) -> Result<Self, Status> {
        let info: DisplayCoordinatorGetInfoResponse =
            call(&mut display, 1, &DisplayCoordinatorGetInfoRequest {})?;
        if info.status != Status::Ok {
            return Err(info.status);
        }
        let surface = surface(info.surface).map_err(|_| Status::ErrInvalidArgs)?;
        let len = surface
            .validate(u64::MAX)
            .map_err(|_| Status::ErrInvalidArgs)?;
        let output = Mapping::new(len as u64).map_err(|_| Status::ErrNoMemory)?;
        let generation = if acquire {
            let r: DisplayCoordinatorAcquireResponse =
                call(&mut display, 2, &DisplayCoordinatorAcquireRequest {})?;
            if r.status != Status::Ok {
                return Err(r.status);
            }
            r.generation
        } else {
            0
        };
        Ok(Self {
            display,
            surface,
            output,
            generation,
            client_id: info.client_id,
        })
    }
    pub fn present(&mut self) -> Result<u64, Status> {
        self.present_damage(self.surface.full())
    }
    pub fn present_damage(&mut self, d: bexos_graphics::Damage) -> Result<u64, Status> {
        d.validate(self.surface)
            .map_err(|_| Status::ErrInvalidArgs)?;
        let h =
            Memory::duplicate(self.output.handle, 1 | 2 | 16 | 32).map_err(|_| Status::ErrIo)?;
        let r: DisplayCoordinatorPresentResponse = call(
            &mut self.display,
            3,
            &DisplayCoordinatorPresentRequest {
                generation: self.generation,
                buffer: HandleRef { raw: h },
                surface: wire(self.surface),
                damage: Damage {
                    x: d.x,
                    y: d.y,
                    width: d.width,
                    height: d.height,
                },
            },
        )?;
        if r.status != Status::Ok {
            return Err(r.status);
        }
        Ok(r.fence)
    }
    pub fn encode(&self, w: &mut Encoder) {
        w.word(self.display.0);
        w.word(self.surface.width as u64);
        w.word(self.surface.height as u64);
        w.word(self.surface.stride as u64);
        w.word(self.surface.format as u64);
        self.output.encode(w);
        w.word(self.generation);
        w.word(self.client_id);
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        let display = Channel(r.word()?);
        let surface = Surface {
            width: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
            height: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
            stride: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
            format: (r.word()? as u32)
                .try_into()
                .map_err(|_| Error::InvalidData)?,
        };
        let output = Mapping::decode(r)?;
        surface
            .validate(output.size)
            .map_err(|_| Error::InvalidData)?;
        let generation = r.word()?;
        let client_id = r.word()?;
        if display.0 == 0 || client_id == 0 {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            display,
            surface,
            output,
            generation,
            client_id,
        })
    }
    pub fn resources(&self) -> Vec<Resource> {
        let mut out = self.output.resources();
        out.push(Resource::Handle(self.display.0));
        out
    }
}
