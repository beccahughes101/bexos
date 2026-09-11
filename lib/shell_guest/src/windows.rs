//! Serializable window geometry; native view handles stay in the guest resource table.
use bexos_dioxus_guest::views::ChildView;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
#[derive(Clone, Debug)]
pub struct Window {
    pub view: ChildView,
    pub node: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl Window {
    pub fn constrain(&mut self, w: u32, h: u32) {
        self.width = self.width.clamp(160, w.max(160));
        self.height = self.height.clamp(100, h.saturating_sub(48).max(100));
        self.x = self.x.clamp(0, w.saturating_sub(self.width) as i32);
        self.y = self.y.clamp(0, h.saturating_sub(48 + self.height) as i32);
    }
    pub fn encode(&self, w: &mut Encoder) {
        for n in [
            self.view.id,
            self.view.token as u64,
            self.view.width as u64,
            self.view.height as u64,
            self.node,
            self.x as i64 as u64,
            self.y as i64 as u64,
            self.width as u64,
            self.height as u64,
        ] {
            w.word(n);
        }
        w.text(&self.view.package);
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        let id = r.word()?;
        let token = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let vw = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let vh = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let node = r.word()?;
        let x = r.word()? as i64;
        let y = r.word()? as i64;
        let width = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let height = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let package = r.text(128)?.into();
        if id == 0
            || token == 0
            || node == 0
            || width < 160
            || height < 100
            || width > 8192
            || height > 8192
        {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            view: ChildView {
                id,
                token,
                width: vw,
                height: vh,
                focused: false,
                package,
            },
            node,
            x: x.try_into().map_err(|_| Error::InvalidData)?,
            y: y.try_into().map_err(|_| Error::InvalidData)?,
            width,
            height,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resize_and_checkpoint() {
        let mut v = Window {
            view: ChildView {
                id: 1,
                token: 2,
                package: "app".into(),
                width: 640,
                height: 420,
                focused: false,
            },
            node: 10,
            x: 790,
            y: 590,
            width: 1000,
            height: 1000,
        };
        v.constrain(800, 600);
        assert_eq!((v.x, v.y, v.width, v.height), (0, 0, 800, 552));
        let mut w = Encoder::new();
        v.encode(&mut w);
        let bytes = w.finish();
        let mut r = Decoder::new(&bytes);
        let restored = Window::decode(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(restored.width, 800);
        assert_eq!(restored.view.id, 1);
    }
}
