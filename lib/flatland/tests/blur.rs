use bexos_flatland::{
    Damage, Format, Surface,
    blur::{BoxBlur, MAX_RADIUS},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};
struct Allocator;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(p, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

#[test]
fn bounded_in_place_blur_matches_square_reference_without_frame_allocations() {
    for (width, height) in [(1, 1), (7, 5), (23, 17)] {
        for format in [Format::Rgba, Format::Bgra] {
            let surface = Surface {
                width,
                height,
                stride: width * 4 + 12,
                format,
            };
            let mut original = vec![0xa5; surface.validate(u64::MAX).unwrap()];
            for y in 0..height {
                for x in 0..width {
                    let offset = (y * surface.stride + x * 4) as usize;
                    original[offset..offset + 4].copy_from_slice(&[
                        ((x * 17 + y * 7) % 200) as u8,
                        ((x * 3 + y * 29) % 200) as u8,
                        ((x * 23 + y * 13) % 200) as u8,
                        200,
                    ]);
                }
            }
            let mut blur = BoxBlur::new(width).unwrap();
            for output in [
                surface.full(),
                Damage {
                    x: width / 2,
                    y: height / 2,
                    width: width - width / 2,
                    height: height - height / 2,
                },
            ] {
                for radius in [0, 1, 3, MAX_RADIUS] {
                    let mut actual = original.clone();
                    let before = ALLOCATIONS.load(Ordering::Relaxed);
                    blur.apply(surface, &mut actual, output, radius).unwrap();
                    assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), before);
                    for y in 0..height {
                        for x in 0..surface.stride {
                            let offset = (y * surface.stride + x) as usize;
                            if x >= output.x * 4
                                && x < (output.x + output.width) * 4
                                && y >= output.y
                                && y < output.y + output.height
                            {
                                let mut sum = 0u32;
                                let channel = x % 4;
                                for dy in -(radius as i64)..=radius as i64 {
                                    for dx in -(radius as i64)..=radius as i64 {
                                        let sx =
                                            (x as i64 / 4 + dx).clamp(0, width as i64 - 1) as u32;
                                        let sy = (y as i64 + dy).clamp(0, height as i64 - 1) as u32;
                                        sum += original
                                            [(sy * surface.stride + sx * 4 + channel) as usize]
                                            as u32;
                                    }
                                }
                                let count = (radius * 2 + 1).pow(2);
                                let expected = ((sum + count / 2) / count) as u8;
                                assert!(
                                    actual[offset].abs_diff(expected) <= 1,
                                    "({x},{y}) r={radius}: {} != {expected}",
                                    actual[offset]
                                );
                            } else {
                                assert_eq!(
                                    actual[offset], original[offset],
                                    "outside output or row padding"
                                );
                            }
                        }
                    }
                }
            }
            let before = original.clone();
            assert!(
                blur.apply(surface, &mut original, surface.full(), MAX_RADIUS + 1)
                    .is_err()
            );
            assert_eq!(original, before);
        }
    }
}
