use bexos_dioxus_guest::locale::{self, LocaleContext, t};
const STRINGS: &[u8] = include_bytes!(env!("BEXOS_STRINGS"));
use bexos_dioxus_guest::exports::bexos::wasm::lifecycle::Guest;
use bexos_dioxus_guest::{AssetKind, View, dom};
use std::cell::RefCell;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 420;
const FONT: u32 = 1;
const IMAGE: u32 = 2;

pub struct Service;

#[derive(Default)]
struct State {
    locale: Option<LocaleContext>,
    initialized: bool,
    view: Option<View>,
    counter: u32,
    text: String,
    scroll: f32,
    backend: String,
    failure: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        locale: None,
        initialized: false,
        view: None,
        counter: 0,
        text: String::new(),
        scroll: 0.0,
        backend: "starting".into(),
        failure: String::new(),
    });
}

impl Guest for Service {
    fn dispatch(_resource_id: u32) {
        STATE.with_borrow_mut(|state| {
            let initial = !state.initialized;
            if let Err(error) = locale::poll(&mut state.locale, STRINGS) {
                state.failure = error;
                return;
            }
            if initial && state.text.is_empty() && state.view.is_none() {
                state.text = t!(state.locale.as_ref().unwrap(), "edit");
            }
            state.initialized = true;
            if state.view.is_none() {
                match View::open(WIDTH, HEIGHT) {
                    Ok(view) => {
                        let _ = view.register_asset(FONT, AssetKind::Font, b"packaged-demo-font");
                        let _ =
                            view.register_asset(IMAGE, AssetKind::Image, b"packaged-demo-image");
                        state.view = Some(view);
                    }
                    Err(error) => {
                        state.failure = error;
                        return;
                    }
                }
            }
            let Some(view) = &state.view else {
                return;
            };
            if let Ok(events) = view.input() {
                for event in events {
                    if event.kind == 1 && event.phase == 1 {
                        state.counter = state.counter.saturating_add(1);
                    }
                    if event.kind == 2 && event.key_state == 1 {
                        match event.unicode {
                            8 | 127 => {
                                state.text.pop();
                            }
                            32..=0x10ffff => {
                                if state.text.len() < 64 {
                                    if let Some(ch) = char::from_u32(event.unicode) {
                                        state.text.push(ch);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    if event.kind == 3 {
                        state.scroll = (state.scroll + event.scroll_y).clamp(-160.0, 160.0);
                    }
                }
            }
            if let Ok(backend) = view.backend() {
                state.backend = backend.backend;
                state.failure = backend.failure.unwrap_or_default();
            }
            let _ = view.submit_document(&render(state));
        });
    }

    fn checkpoint() -> Vec<u8> {
        STATE.with_borrow(|state| {
            let mut out = Vec::new();
            out.extend_from_slice(&state.counter.to_le_bytes());
            out.extend_from_slice(&state.scroll.to_le_bytes());
            out.extend_from_slice(&(state.text.len() as u32).to_le_bytes());
            out.extend_from_slice(state.text.as_bytes());
            out.extend_from_slice(&state.view.as_ref().map_or(0, |v| v.id()).to_le_bytes());
            out
        })
    }

    fn restore(checkpoint: Vec<u8>) -> Result<(), ()> {
        if checkpoint.len() < 12 {
            return Err(());
        }
        let counter = u32::from_le_bytes(checkpoint[0..4].try_into().map_err(|_| ())?);
        let scroll = f32::from_le_bytes(checkpoint[4..8].try_into().map_err(|_| ())?);
        let len = u32::from_le_bytes(checkpoint[8..12].try_into().map_err(|_| ())?) as usize;
        let text_bytes = checkpoint.get(12..12 + len).ok_or(())?;
        let text = std::str::from_utf8(text_bytes).map_err(|_| ())?.to_string();
        let tail = checkpoint.get(12 + len..).ok_or(())?;
        let view = if tail.is_empty() {
            0
        } else if tail.len() == 4 {
            u32::from_le_bytes(tail.try_into().map_err(|_| ())?)
        } else {
            return Err(());
        };
        if !scroll.is_finite() || text.len() > 64 {
            return Err(());
        }
        STATE.with_borrow_mut(|state| {
            state.locale = None;
            state.initialized = true;
            state.view = (view != 0).then(|| View::adopt(view));
            state.counter = counter;
            state.scroll = scroll;
            state.text = text;
        });
        Ok(())
    }

    fn activate() {}

    fn abort() {}
}

fn render(state: &State) -> dom::Document {
    let locale = state.locale.as_ref().unwrap();
    let backend_color = if state.backend == "gpu" {
        "#27ae60"
    } else {
        "#f39c12"
    };
    dom::Document {
        version: dom::VERSION,
        width: WIDTH,
        height: HEIGHT,
        scale: 1.0,
        clear_rgba: [32, 37, 45, 255],
        author_stylesheets: Vec::new(),
        stylesheets: vec![
            dom::StyleRule {
                selector: ".root".into(),
                declarations: "width:592px;height:372px;display:flex;flex-direction:column;gap:14px;padding:20px;background:#f5f7fa".into(),
                origin: dom::StyleOrigin::Author,
            },
            dom::StyleRule {
                selector: ".top".into(),
                declarations: "height:86px;display:flex;flex-direction:row;gap:22px".into(),
                origin: dom::StyleOrigin::Author,
            },
            dom::StyleRule {
                selector: ".counter".into(),
                declarations: "background:#34495e;color:#ffffff;font-size:22px;padding:16px".into(),
                origin: dom::StyleOrigin::Author,
            },
            dom::StyleRule {
                selector: ".editor".into(),
                declarations: "background:#ecf0f1;color:#2c3e50;font-size:20px;padding:16px".into(),
                origin: dom::StyleOrigin::Author,
            },
            dom::StyleRule {
                selector: ".content".into(),
                declarations: format!("height:150px;display:grid;grid-template-columns:1fr 1fr;gap:12px;padding:16px;background:#ffffff;top:{}px", state.scroll),
                origin: dom::StyleOrigin::Author,
            },
            dom::StyleRule {
                selector: ".tile".into(),
                declarations: "background:#dfe6e9;color:#2c3e50;font-size:18px;padding:12px".into(),
                origin: dom::StyleOrigin::Author,
            },
            dom::StyleRule {
                selector: ".status".into(),
                declarations: format!("height:36px;background:{backend_color};color:#ffffff;font-size:18px;padding:8px"),
                origin: dom::StyleOrigin::Author,
            },
        ],
        root: dom::Node::element(1, "main").class("root").attribute("lang", &locale.settings().languages[0]).attribute("dir", locale.direction()).style(format!("direction:{}", locale.direction())).children([
            dom::Node::element(2, "section").class("top").children([
                dom::Node::element(3, "button")
                    .class("counter")
                    .listeners(dom::LISTENER_POINTER)
                    .child(dom::Node::text(4, t!(locale, "counter", count = state.counter))),
                dom::Node::element(5, "input")
                    .class("editor")
                    .listeners(dom::LISTENER_KEYBOARD | dom::LISTENER_TEXT | dom::LISTENER_FOCUS)
                    .child(dom::Node::text(6, state.text.clone())),
            ]),
            dom::Node::element(7, "section")
                .class("content")
                .listeners(dom::LISTENER_WHEEL)
                .children([
                    dom::Node::element(8, "article")
                        .class("tile")
                        .child(dom::Node::text(9, t!(locale, "layout"))),
                    dom::Node::image(10, IMAGE).style("background:#ffffff"),
                    dom::Node::element(11, "article")
                        .class("tile")
                        .child(dom::Node::text(12, t!(locale, "wasm"))),
                    dom::Node::element(13, "article")
                        .class("tile")
                        .child(dom::Node::text(14, t!(locale, "renderer"))),
                ]),
            dom::Node::element(15, "footer").class("status").child(dom::Node::text(
                16,
                if state.failure.is_empty() {
                    t!(locale, "backend", name = state.backend.as_str())
                } else {
                    t!(locale, "backend-failure", name = state.backend.as_str(), error = state.failure.as_str())
                },
            )),
        ]),
    }
}
