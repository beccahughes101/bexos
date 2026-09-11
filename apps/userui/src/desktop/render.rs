use super::*;
impl Desktop {
    pub(super) fn render(&mut self) -> Result<(), String> {
        let view = self.view.as_ref().unwrap();

        let mut frame = paint::scene(self.width, self.height, [27, 48, 68, 255]);
        paint::text(&mut frame, 24.0, 24.0, "BEXOS", 3.0, [85, 115, 140, 255]);
        let y = self.height as f32 - 44.0;
        paint::rect(
            &mut frame,
            0.0,
            y,
            self.width as f32,
            44.0,
            [18, 25, 38, 255],
        );
        paint::button(&mut frame, 8.0, y + 6.0, 96.0, "APPS", self.launcher);
        paint::button(
            &mut frame,
            self.width as f32 - 100.0,
            y + 6.0,
            92.0,
            "LOCK",
            false,
        );
        paint::button(
            &mut frame,
            self.width as f32 - 212.0,
            y + 6.0,
            104.0,
            "LOG OUT",
            false,
        );
        let window_count = self.windows.len();
        for (i, w) in self.windows.iter_mut().enumerate() {
            w.constrain(self.width, self.height);
            view.translate_node(w.node, w.x, w.y)?;
            view.embed_at(
                w.node,
                w.node + 1,
                &w.view,
                4,
                TITLE_HEIGHT as i32,
                w.width - 8,
                w.height - VERTICAL_CHROME,
                false,
            )?;
            view.order_child(w.node, i as u32)?;
            let active = i + 1 == window_count;
            let mut decoration = paint::scene(
                w.width,
                w.height,
                if active {
                    [60, 92, 136, 255]
                } else {
                    [43, 56, 76, 255]
                },
            );
            let title = w.view.package.rsplit('.').next().unwrap_or("APP");
            paint::text(&mut decoration, 10.0, 9.0, title, 1.5, paint::INK);
            paint::text(
                &mut decoration,
                w.width as f32 - 24.0,
                9.0,
                "X",
                2.0,
                paint::INK,
            );
            for inset in [5.0, 9.0] {
                paint::rect(
                    &mut decoration,
                    w.width as f32 - 12.0,
                    w.height as f32 - inset,
                    8.0,
                    2.0,
                    paint::INK,
                );
            }
            view.set_node_scene(w.node, &decoration)?;
            if 112 + i as u32 * 120 + 120 < self.width.saturating_sub(212) {
                paint::button(
                    &mut frame,
                    112.0 + i as f32 * 120.0,
                    y + 6.0,
                    116.0,
                    &title.chars().take(9).collect::<String>(),
                    active,
                );
            }
        }
        // The launcher is a compositor child after every window, so it also owns hit testing.
        if self.launcher {
            let mut menu = paint::scene(
                340,
                (self.snapshot.apps.len().max(1).min(10) as u32 * 38 + 40).min(self.height - 48),
                [28, 38, 56, 255],
            );
            if self.snapshot.apps.is_empty() {
                paint::text(&mut menu, 16.0, 24.0, "NO GRAPHICAL APPS", 2.0, paint::INK);
            }
            for (i, (_, name)) in self
                .snapshot
                .apps
                .iter()
                .skip(self.launcher_offset)
                .take(10)
                .enumerate()
            {
                paint::button(
                    &mut menu,
                    8.0,
                    20.0 + i as f32 * 38.0,
                    324.0,
                    &name.chars().take(25).collect::<String>(),
                    false,
                );
            }
            // Reuse the menu node while open; closing releases it below.
            let _ = view.create_node(50, view.root_node());
            view.set_node_scene(50, &menu)?;
            view.order_child(50, self.windows.len() as u32)?;
        } else {
            let _ = view.remove_child(50);
        }
        if !self.error.is_empty() {
            paint::text(
                &mut frame,
                24.0,
                self.height as f32 - 66.0,
                &self.error,
                1.0,
                [255, 180, 170, 255],
            );
        }
        view.submit(&frame)?;
        self.dirty = false;

        Ok(())
    }
}
