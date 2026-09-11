//! Nonblocking device subscription. Late drivers cannot stall frame deadlines.
use crate::input::Link;
use bexos_graphics_runtime as rt;
use input_fidl::FidlDecode;
use kernel_fidl::Status;
impl Link {
    pub fn poll_subscription(&mut self, now: u64) -> Result<bool, Status> {
        let Some(deadline) = self.subscription_deadline else {
            return Ok(true);
        };
        let mut storage = [0; 1024];
        let bytes = match rt::stream::read_no_handles(self.control, &mut storage) {
            Ok(bytes) => bytes,
            Err(Status::ErrTimedOut) if now < deadline => return Ok(false),
            Err(error) => return Err(error),
        };
        let reply = input_fidl::InputDeviceSubscribeResponse::decode(bytes, &[])
            .map_err(|_| Status::ErrInvalidArgs)?;
        if reply.status != input_fidl::Status::Ok {
            return Err(Status::ErrInvalidArgs);
        }
        self.device.axes.copy_from_slice(&reply.axes[..4]);
        let touch = &mut self.device.touch;
        touch.min_x = reply.axes[4];
        touch.max_x = reply.axes[5];
        touch.min_y = reply.axes[6];
        touch.max_y = reply.axes[7];
        touch.enabled = touch.max_x > touch.min_x && touch.max_y > touch.min_y;
        self.subscription_deadline = None;
        bexos_userspace::log(&format!(
            "scened: input device attached node={} endpoint={}\n",
            reply.node, self.control.0
        ));
        Ok(true)
    }
}
