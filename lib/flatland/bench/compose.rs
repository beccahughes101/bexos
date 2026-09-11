use bexos_flatland::{resolved::Snapshot, scene::Graph, *};
mod allocations;
mod measurement;
fn main() {
    let target = Surface {
        width: 1920,
        height: 1080,
        stride: 7680,
        format: Format::Bgra,
    };
    for (name, count, size, partial) in [
        ("fullscreen_cpu_1920x1080", 1, 1920, false),
        ("animated_overlap_8x640x480", 8, 640, false),
        ("partial_damage_64x64", 8, 640, true),
    ] {
        let mut graph = Graph::default();
        graph.create(1).unwrap();
        graph.root = Some(1);
        let height = if size == 1920 { 1080 } else { 480 };
        let source = Surface {
            width: size,
            height,
            stride: size * 4,
            format: Format::Bgra,
        };
        for n in 0..count {
            let id = n + 2;
            graph.create(id).unwrap();
            let node = graph.node(id).unwrap();
            node.translation = ((n * 100) as i32, (n * 50) as i32);
            node.content = Some((1, source));
            if count > 1 {
                node.opacity = 0.8;
            }
            graph.attach(1, id).unwrap();
        }
        let mut snapshot = Snapshot::default();
        snapshot.compile(&graph, target).unwrap();
        let mut pixels = vec![255; (source.stride * source.height) as usize];
        if count > 1 {
            // Premultiplied translucent content exercises real blending rather
            // than eight opaque memcpy operations that happen to overlap.
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[64, 96, 128, 192]);
            }
        }
        let mut output = vec![0; (target.stride * target.height) as usize];
        let damage = if partial {
            Damage {
                x: 400,
                y: 300,
                width: 64,
                height: 64,
            }
        } else {
            target.full()
        };
        measurement::measure(
            name,
            damage.width as u64 * damage.height as u64 * 4,
            |frame| {
                if count > 1 && !partial {
                    graph.node(2).unwrap().translation = ((frame % 100) as i32, 0);
                    snapshot.compile(&graph, target).unwrap();
                }
                composition::composite(&snapshot, target, &mut output, damage, |_| {
                    Some(pixels.as_slice())
                })
                .unwrap();
                std::hint::black_box(&output);
            },
        );
        if count == 1 {
            graph.node(2).unwrap().content.as_mut().unwrap().1.format = Format::Bgrx;
            snapshot.compile(&graph, target).unwrap();
            measurement::measure("fullscreen_scanout_eligibility_1920x1080", 0, |_| {
                std::hint::black_box(
                    scanout::candidate(
                        snapshot.items.iter(),
                        target,
                        1 << Format::Bgrx as u32,
                        false,
                    )
                    .unwrap(),
                );
            });
        }
    }
}
