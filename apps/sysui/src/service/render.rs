use super::*;
use bexos_ui_prelude::*;

impl State {
    pub(super) fn render(&mut self) -> Result<(), String> {
        let view = self.view.as_ref().unwrap();
        let x = (self.width as f32 - 440.0) / 2.0;
        let y = (self.height as f32 - 280.0) / 2.0;

        let mut root = Node::element(1, "main").class(CLASS_ROOT).style(format!(
            "width:{}px;height:{}px;background:var(--bex-bg);",
            self.width, self.height
        ));
        if self.snapshot.state != 1 {
            root = root.child(
                Node::element(2, "section")
                    .class(CLASS_PANEL)
                    .style(format!("x:{x}px;y:{y}px;width:440px;height:280px;")),
            );
            root.children[0] = root.children[0].clone().child(label(
                3,
                0.0,
                0.0,
                240.0,
                52.0,
                "BEXOS",
                CLASS_TITLE,
            ));
            if self.snapshot.users.is_empty() {
                root.children[0] = root.children[0]
                    .clone()
                    .child(label(
                        4,
                        0.0,
                        70.0,
                        440.0,
                        24.0,
                        "NO USERS CONFIGURED",
                        CLASS_ERROR,
                    ))
                    .child(label(
                        5,
                        0.0,
                        104.0,
                        440.0,
                        24.0,
                        "USE BEXCTL USERS CREATE TO SET UP",
                        CLASS_MUTED,
                    ));
            } else {
                let label_text = self
                    .snapshot
                    .users
                    .iter()
                    .find(|(uid, _)| *uid == self.snapshot.uid)
                    .or_else(|| self.snapshot.users.get(self.selected))
                    .map(|(_, name)| name.as_str())
                    .unwrap_or("USER");
                root.children[0] = root.children[0]
                    .clone()
                    .child(
                        Button::new(10, label_text)
                            .primary(true)
                            .node()
                            .style("x:0px;y:56px;width:440px;height:32px;"),
                    )
                    .child(
                        TextInput::new(20)
                            .value(self.password.clone())
                            .placeholder("PASSWORD")
                            .masked(true)
                            .error(!self.error.is_empty())
                            .node()
                            .style("x:0px;y:108px;width:440px;height:36px;"),
                    )
                    .child(
                        Button::new(
                            30,
                            if self.snapshot.uid == 0 {
                                "SIGN IN"
                            } else {
                                "UNLOCK"
                            },
                        )
                        .primary(true)
                        .node()
                        .style("x:0px;y:164px;width:180px;height:32px;"),
                    )
                    .child(label(40, 0.0, 218.0, 420.0, 24.0, &self.error, CLASS_ERROR));
            }
            if self.snapshot.uid != 0 {
                root.children[0] = root.children[0].clone().child(
                    Button::new(50, "LOG OUT")
                        .node()
                        .style("x:200px;y:164px;width:180px;height:32px;"),
                );
            }
            root = root.child(label(
                60,
                20.0,
                self.height as f32 - 24.0,
                self.width as f32 - 40.0,
                16.0,
                &self.snapshot.diagnostic,
                CLASS_MUTED,
            ));
        }
        let document = Document {
            version: bexos_dioxus_guest::dom::VERSION,
            width: self.width,
            height: self.height,
            scale: 1.0,
            clear_rgba: [18, 26, 42, 255],
            stylesheets: vec![],
            author_stylesheets: vec![component_css()],
            root,
        };
        view.submit_document(&document)?;
        self.dirty = false;

        Ok(())
    }
}

fn label(id: u64, x: f32, y: f32, width: f32, height: f32, value: &str, class: &str) -> Node {
    Node::element(id, "div")
        .class(class)
        .style(format!(
            "x:{x}px;y:{y}px;width:{width}px;height:{height}px;"
        ))
        .child(Node::text(id + 1, value))
}
