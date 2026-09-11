use super::*;
impl State {
    pub(super) fn render(&mut self) -> Result<(), String> {
        let view = self.view.as_ref().unwrap();
        let x = (self.width as f32 - 440.0) / 2.0;
        let y = (self.height as f32 - 280.0) / 2.0;

        let mut frame = paint::scene(self.width, self.height, [18, 26, 42, 255]);
        if self.snapshot.state != 1 {
            paint::text(&mut frame, x, y, "BEXOS", 4.0, paint::INK);
            if self.snapshot.users.is_empty() {
                paint::text(
                    &mut frame,
                    x,
                    y + 64.0,
                    "NO USERS CONFIGURED",
                    2.0,
                    paint::INK,
                );
                paint::text(
                    &mut frame,
                    x,
                    y + 96.0,
                    "USE BEXCTL USERS CREATE TO SET UP",
                    2.0,
                    paint::INK,
                );
            } else {
                let label = self
                    .snapshot
                    .users
                    .iter()
                    .find(|(uid, _)| *uid == self.snapshot.uid)
                    .or_else(|| self.snapshot.users.get(self.selected))
                    .map(|(_, name)| name.as_str())
                    .unwrap_or("USER");
                paint::button(&mut frame, x, y + 56.0, 440.0, label, true);
                paint::rect(&mut frame, x, y + 108.0, 440.0, 36.0, [35, 48, 68, 255]);
                let mask = "*".repeat(self.password.chars().count().min(32));
                paint::text(
                    &mut frame,
                    x + 10.0,
                    y + 118.0,
                    if mask.is_empty() { "PASSWORD" } else { &mask },
                    2.0,
                    paint::INK,
                );
                paint::button(
                    &mut frame,
                    x,
                    y + 164.0,
                    180.0,
                    if self.snapshot.uid == 0 {
                        "SIGN IN"
                    } else {
                        "UNLOCK"
                    },
                    true,
                );
                paint::text(
                    &mut frame,
                    x,
                    y + 218.0,
                    &self.error,
                    1.5,
                    [255, 166, 158, 255],
                );
            }
            if self.snapshot.uid != 0 {
                paint::button(&mut frame, x + 200.0, y + 164.0, 180.0, "LOG OUT", false);
            }
            paint::text(
                &mut frame,
                20.0,
                self.height as f32 - 24.0,
                &self.snapshot.diagnostic,
                1.0,
                paint::INK,
            );
        }
        view.submit(&frame)?;
        self.dirty = false;

        Ok(())
    }
}
