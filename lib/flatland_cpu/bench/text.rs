//! Host CPU text workloads; neither display latency nor hardware acceptance.
use bexos_flatland::{Format, Surface};
use bexos_flatland_cpu::Rasterizer;
use bexos_flatland_text::{TextEngine, TextStyle};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(size, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn measure(name: &str, mut operation: impl FnMut(usize)) {
    assert!(
        !cfg!(debug_assertions),
        "run benchmarks with bazel run -c opt"
    );
    for i in 0..10 {
        operation(i);
    }
    let mut times = [0u128; 200];
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes_before = ALLOCATED_BYTES.load(Ordering::Relaxed);
    for (i, time) in times.iter_mut().enumerate() {
        let begin = Instant::now();
        operation(i + 10);
        *time = begin.elapsed().as_nanos();
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    let allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed) - bytes_before;
    times.sort_unstable();
    println!(
        "{{\"workload\":\"{name}\",\"scope\":\"host operation wall time\",\"samples\":200,\"warmup\":10,\"p50_ns\":{},\"p99_ns\":{},\"allocations\":{allocations},\"allocated_bytes\":{allocated_bytes},\"hardware_performance_verified\":false}}",
        times[99], times[197]
    );
}
fn main() {
    let mut engine = TextEngine::default();
    for font in [
        include_bytes!(env!("NOTO_SANS")).as_slice(),
        include_bytes!(env!("NOTO_ARABIC")).as_slice(),
        include_bytes!(env!("NOTO_DEVANAGARI")).as_slice(),
    ] {
        engine.register_font(font.to_vec()).unwrap();
    }
    let style = TextStyle {
        width: 480.,
        size: 24.,
        ..Default::default()
    };
    let text = "BexOS\nمرحبا بالعالم\nनमस्ते दुनिया";
    let layout = engine.shape(text, style.clone()).unwrap();
    let mut raster = Rasterizer::new(Surface {
        width: 480,
        height: 120,
        stride: 1920,
        format: Format::Rgba,
    })
    .unwrap();
    measure("cached_multilingual_shape", |_| {
        std::hint::black_box(engine.shape(text, style.clone()).unwrap());
    });
    measure("cached_glyph_raster_480x120", |_| {
        std::hint::black_box(raster.text(&layout, 0., 0.).unwrap());
    });
    let dynamic: Vec<_> = (0..210)
        .map(|i| format!("BexOS frame {i:04}\nمرحبا بالعالم\nनमस्ते दुनिया"))
        .collect();
    measure("changing_multilingual_shape_and_raster", |i| {
        let layout = engine.shape(&dynamic[i], style.clone()).unwrap();
        std::hint::black_box(raster.text(&layout, 0., 0.).unwrap());
    });
    println!(
        "scope=host CPU only; font loading, destination copies, IPC, GPU and display timing excluded"
    );
}
