use super::*;
use bexos_ui_prelude::*;

impl Desktop {
    pub(super) fn render(&mut self) -> Result<(), String> {
        let locale = self.locale.as_ref().ok_or("locale unavailable")?;
        let tr = |key| locale.text(key, &Default::default());
        let view = self.view.as_ref().unwrap();

        let mut root = Node::element(1, "main")
            .class(CLASS_ROOT)
            .attribute("lang", &locale.settings().languages[0])
            .attribute("dir", locale.direction())
            .style(format!(
                "width:{}px;height:{}px;background:var(--bex-bg);direction:{};",
                self.width,
                self.height,
                locale.direction()
            ))
            .child(label(2, 24.0, 24.0, 180.0, 38.0, &tr("brand"), CLASS_TITLE));
        let y = self.height as f32 - 44.0;
        root = root
            .child(
                Node::element(10, "section")
                    .class(CLASS_PANEL)
                    .style(format!(
                        "x:0px;y:{y}px;width:{}px;height:44px;background:#121926;",
                        self.width
                    )),
            )
            .child(
                Button::new(20, &tr("apps"))
                    .active(self.launcher)
                    .node()
                    .style(format!("x:8px;y:{}px;width:96px;height:32px;", y + 6.0)),
            )
            .child(Button::new(30, &tr("lock")).node().style(format!(
                "x:{}px;y:{}px;width:92px;height:32px;",
                self.width as f32 - 100.0,
                y + 6.0
            )))
            .child(Button::new(40, &tr("log-out")).node().style(format!(
                "x:{}px;y:{}px;width:104px;height:32px;",
                self.width as f32 - 212.0,
                y + 6.0
            )));
        let fallback_app = tr("app");
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
            let title = w.view.package.rsplit('.').next().unwrap_or(&fallback_app);
            root = root.child(window_chrome(
                1000 + i as u64 * 10,
                w,
                title,
                active,
                &tr("close"),
            ));
            if 112 + i as u32 * 120 + 120 < self.width.saturating_sub(212) {
                root = root.child(
                    Button::new(
                        2000 + i as u64 * 2,
                        title.chars().take(9).collect::<String>(),
                    )
                    .active(active)
                    .node()
                    .style(format!(
                        "x:{}px;y:{}px;width:116px;height:32px;",
                        112.0 + i as f32 * 120.0,
                        y + 6.0
                    )),
                );
            }
        }
        if self.launcher {
            let menu_height =
                (self.snapshot.apps.len().max(1).min(10) as u32 * 38 + 40).min(self.height - 48);
            let mut menu = Node::element(3000, "section")
                .class(CLASS_PANEL)
                .style(format!(
                    "x:0px;y:0px;width:340px;height:{menu_height}px;background:var(--bex-panel);"
                ));
            if self.snapshot.apps.is_empty() {
                menu = menu.child(label(
                    3001,
                    16.0,
                    24.0,
                    300.0,
                    24.0,
                    &tr("no-apps"),
                    CLASS_MUTED,
                ));
            }
            for (i, (_, name)) in self
                .snapshot
                .apps
                .iter()
                .skip(self.launcher_offset)
                .take(10)
                .enumerate()
            {
                menu = menu.child(
                    Button::new(
                        3020 + i as u64 * 2,
                        name.chars().take(25).collect::<String>(),
                    )
                    .node()
                    .style(format!(
                        "x:8px;y:{}px;width:324px;height:32px;",
                        20.0 + i as f32 * 38.0
                    )),
                );
            }
            root = root.child(menu);
        }
        if !self.error.is_empty() {
            root = root.child(label(
                4000,
                24.0,
                self.height as f32 - 66.0,
                self.width as f32 - 48.0,
                18.0,
                &self.error,
                CLASS_ERROR,
            ));
        }
        let document = Document {
            version: bexos_dioxus_guest::dom::VERSION,
            width: self.width,
            height: self.height,
            scale: 1.0,
            clear_rgba: [27, 48, 68, 255],
            stylesheets: vec![],
            author_stylesheets: vec![component_css()],
            root,
        };
        view.submit_document(&document)?;
        self.dirty = false;

        Ok(())
    }
}

fn window_chrome(
    id: u64,
    w: &bexos_shell_guest::windows::Window,
    title: &str,
    active: bool,
    close: &str,
) -> Node {
    Node::element(id, "section")
        .class(CLASS_PANEL)
        .style(format!(
            "x:{}px;y:{}px;width:{}px;height:{}px;background:{};",
            w.x,
            w.y,
            w.width,
            w.height,
            if active { "#3c5c88" } else { "#2b384c" }
        ))
        .child(label(id + 1, 10.0, 9.0, 180.0, 18.0, title, CLASS_MUTED))
        .child(label(
            id + 2,
            w.width as f32 - 24.0,
            9.0,
            16.0,
            18.0,
            close,
            CLASS_MUTED,
        ))
        .child(Node::element(id + 3, "div").style(format!(
            "x:{}px;y:{}px;width:8px;height:2px;background:var(--bex-ink);",
            w.width as f32 - 12.0,
            w.height as f32 - 5.0
        )))
        .child(Node::element(id + 4, "div").style(format!(
            "x:{}px;y:{}px;width:8px;height:2px;background:var(--bex-ink);",
            w.width as f32 - 12.0,
            w.height as f32 - 9.0
        )))
}

fn label(id: u64, x: f32, y: f32, width: f32, height: f32, value: &str, class: &str) -> Node {
    Node::element(id, "div")
        .class(class)
        .style(format!(
            "x:{x}px;y:{y}px;width:{width}px;height:{height}px;"
        ))
        .child(Node::text(id + 1, value))
}
