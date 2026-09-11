use bexos_d1_linux_shim::error::Result;
use bexos_d1_linux_shim::mmio::Mmio;

use crate::spec::{CC_ENABLE, CSTS_READY, REG_CC, REG_CSTS};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerState {
    New,
    Ready,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Controller {
    pub state: ControllerState,
}

impl Controller {
    pub const fn new() -> Self {
        Self {
            state: ControllerState::New,
        }
    }

    pub fn enable<M: Mmio>(&mut self, mmio: &mut M) -> Result<()> {
        mmio.write32(REG_CC, CC_ENABLE)?;
        mmio.write32(REG_CSTS, CSTS_READY)?;
        self.state = ControllerState::Ready;
        Ok(())
    }
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}
