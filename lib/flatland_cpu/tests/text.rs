use bexos_flatland::{Format, Surface};
use bexos_flatland_cpu::Rasterizer;
use bexos_flatland_text::{TextEngine, TextStyle};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
static LARGEST: AtomicUsize = AtomicUsize::new(0);
struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LARGEST.fetch_max(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        LARGEST.fetch_max(size, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
#[test]
fn multilingual_glyphs_render_and_old_text_is_cleared() {
    let mut engine = TextEngine::default();
    for font in [
        include_bytes!(env!("NOTO_SANS")).as_slice(),
        include_bytes!(env!("NOTO_ARABIC")).as_slice(),
        include_bytes!(env!("NOTO_DEVANAGARI")).as_slice(),
    ] {
        engine.register_font(font.to_vec()).unwrap();
    }
    let surface = Surface {
        width: 400,
        height: 100,
        stride: 1600,
        format: Format::Bgra,
    };
    LARGEST.store(0, Ordering::Relaxed);
    let mut raster = Rasterizer::new(surface).unwrap();
    let mut output = vec![0; 400 * 100 * 4];
    for text in ["BexOS ready", "مرحبا بالعالم", "नमस्ते दुनिया"]
    {
        let layout = engine
            .shape(
                text,
                TextStyle {
                    size: 26.,
                    width: 380.,
                    color: [240, 90, 30, 255],
                    ..Default::default()
                },
            )
            .unwrap();
        let pixels = raster.text(&layout, 10., 10.).unwrap();
        assert!(pixels.chunks_exact(4).filter(|p| p[3] > 128).count() > 100);
        raster.copy_to(&mut output).unwrap();
        assert!(output.chunks_exact(4).any(|p| p[2] > p[0] && p[3] > 128));
    }
    let empty = engine.shape("", TextStyle::default()).unwrap();
    assert!(raster.text(&empty, 0., 0.).unwrap().iter().all(|p| *p == 0));
    assert!(
        LARGEST.load(Ordering::Relaxed) < 8 * 1024 * 1024,
        "glyph cache requested an oversized guest allocation"
    );
}
