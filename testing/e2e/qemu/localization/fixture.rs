use bexos_dioxus_guest::{
    View, dom,
    exports::bexos::wasm::lifecycle::Guest,
    locale::{self, LocaleContext, t},
};
use bexos_migration::codec::{Decoder, Encoder};
use std::cell::RefCell;
struct Fixture;
#[derive(Default)]
struct State {
    view: Option<View>,
    locale: Option<LocaleContext>,
    counter: u32,
    text: String,
    scroll: f32,
    dirty: bool,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State{counter:17,text:"retained".into(),..Default::default()});}
const STRINGS: &[u8] = include_bytes!(env!("BEXOS_STRINGS"));
fn main() {}
impl Guest for Fixture {
    fn dispatch(_: u32) {
        STATE.with_borrow_mut(|s| {
            if let Err(e) = s.tick() {
                eprintln!("locale-fixture: failed {e}");
            }
        });
    }
    fn checkpoint() -> Vec<u8> {
        STATE.with_borrow(|s| {
            let mut w = Encoder::new();
            w.word(1);
            w.word(s.view.as_ref().map_or(0, |v| v.id() as u64));
            w.word(s.counter as u64);
            w.text(&s.text);
            w.word(s.scroll.to_bits() as u64);
            w.finish()
        })
    }
    fn restore(bytes: Vec<u8>) -> Result<(), ()> {
        let mut r = Decoder::new(&bytes);
        if r.word().map_err(|_| ())? != 1 {
            return Err(());
        }
        let view = u32::try_from(r.word().map_err(|_| ())?).map_err(|_| ())?;
        let counter = u32::try_from(r.word().map_err(|_| ())?).map_err(|_| ())?;
        let text = r.text(128).map_err(|_| ())?.to_string();
        let scroll = f32::from_bits(u32::try_from(r.word().map_err(|_| ())?).map_err(|_| ())?);
        r.finish().map_err(|_| ())?;
        if !scroll.is_finite() {
            return Err(());
        }
        STATE.with_borrow_mut(|s| {
            *s = State {
                view: (view != 0).then(|| View::adopt(view)),
                counter,
                text,
                scroll,
                dirty: true,
                ..Default::default()
            }
        });
        Ok(())
    }
    fn activate() {}
    fn abort() {}
}
impl State {
    fn tick(&mut self) -> Result<(), String> {
        self.dirty |= locale::poll(&mut self.locale, STRINGS)?;
        if self.view.is_none() {
            self.view = Some(View::open(640, 420)?);
            self.dirty = true;
        }
        let view = self.view.as_ref().unwrap();
        for e in view.input()? {
            if e.kind == 1 && e.phase == 1 {
                self.counter = self.counter.saturating_add(1);
                self.dirty = true;
            }
            if e.kind == 2 && e.key_state == 1 {
                if let Some(c) = char::from_u32(e.unicode).filter(|c| !c.is_control()) {
                    if self.text.len() + c.len_utf8() <= 128 {
                        self.text.push(c);
                        self.dirty = true;
                    }
                }
            }
            if e.kind == 3 {
                self.scroll = (self.scroll + e.scroll_y).clamp(-100., 100.);
                self.dirty = true;
            }
        }
        if !self.dirty {
            return Ok(());
        }
        let locale = self.locale.as_ref().unwrap();
        let title = t!(locale, "hello", count = self.counter);
        let region = t!(locale, "region", value = 1234.5f64);
        let document = dom::Document {
            version: dom::VERSION,
            width: 640,
            height: 420,
            scale: 1.,
            clear_rgba: [240, 240, 240, 255],
            stylesheets: vec![],
            author_stylesheets: vec![],
            root: dom::Node::element(1, "main")
                .attribute("lang", &locale.settings().languages[0])
                .attribute("dir", locale.direction())
                .style(format!(
                    "width:640px;height:420px;direction:{};background:#f0f0f0;color:#202020;",
                    locale.direction()
                ))
                .listeners(
                    dom::LISTENER_POINTER
                        | dom::LISTENER_KEYBOARD
                        | dom::LISTENER_TEXT
                        | dom::LISTENER_FOCUS
                        | dom::LISTENER_WHEEL,
                )
                .children([
                    dom::Node::element(2, "div")
                        .style("x:20px;y:20px;width:590px;height:45px;font-size:28px")
                        .child(dom::Node::text(3, &title)),
                    dom::Node::element(4, "div")
                        .style("x:20px;y:90px;width:590px;height:45px;font-size:24px")
                        .child(dom::Node::text(5, &region)),
                    dom::Node::element(6, "input")
                        .style(format!(
                            "x:20px;y:{}px;width:590px;height:45px;font-size:24px",
                            160. + self.scroll
                        ))
                        .child(dom::Node::text(7, &self.text)),
                ]),
        };
        view.submit_document(&document)?;
        eprintln!(
            "locale-fixture: frame language={} generation={} counter={} text={} scroll={} title={} region={}",
            locale.settings().languages[0],
            locale.generation(),
            self.counter,
            self.text,
            self.scroll,
            title,
            region
        );
        self.dirty = false;
        Ok(())
    }
}
bexos_dioxus_guest::export!(Fixture with_types_in bexos_dioxus_guest);
