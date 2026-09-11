use bexos_flatland::{resolved::Snapshot, scene::Graph, *};
fn surface(format: Format) -> Surface {
    Surface {
        width: 4,
        height: 4,
        stride: 16,
        format,
    }
}
fn graph() -> Graph {
    let mut graph = Graph::default();
    graph.create(1).unwrap();
    graph.create(2).unwrap();
    graph.root = Some(1);
    graph.node(1).unwrap().content = Some((10, surface(Format::Rgba)));
    graph.attach(1, 2).unwrap();
    graph.node(2).unwrap().content = Some((11, surface(Format::Bgrx)));
    graph
}
fn pick(graph: &Graph, overlays: bool, formats: u32) -> Option<u64> {
    let mut snapshot = Snapshot::default();
    let display = surface(Format::Bgra);
    snapshot.compile(graph, display).unwrap();
    scanout::candidate(snapshot.items.iter(), display, formats, overlays).map(|item| item.buffer)
}
#[test]
fn only_fullscreen_opaque_top_content_with_supported_layout_can_scan_out() {
    let formats = 1 << Format::Bgrx as u32;
    assert_eq!(pick(&graph(), false, formats), Some(11));
    assert_eq!(pick(&graph(), true, formats), None);
    assert_eq!(pick(&graph(), false, 0), None);
    for change in 0..6 {
        let mut graph = graph();
        let node = graph.node(2).unwrap();
        match change {
            0 => node.translation = (1, 0),
            1 => node.scale = (0.5, 1.),
            2 => node.opacity = 0.5,
            3 => node.clip = Some((3, 4)),
            4 => node.content.as_mut().unwrap().1.format = Format::Bgra,
            5 => node.content.as_mut().unwrap().1.stride = 20,
            _ => unreachable!(),
        }
        assert_eq!(pick(&graph, false, formats), None, "change {change}");
    }
}
#[test]
fn opaque_formats_ignore_padding_alpha_but_preserve_layer_opacity() {
    for (format, expected) in [
        (Format::Bgrx, [15, 10, 55, 255]),
        (Format::Rgbx, [5, 10, 65, 255]),
    ] {
        assert_eq!(Format::try_from(format as u32), Ok(format));
        let mut graph = Graph::default();
        graph.create(1).unwrap();
        graph.root = Some(1);
        let node = graph.node(1).unwrap();
        node.opacity = 0.5;
        node.content = Some((
            10,
            Surface {
                width: 1,
                height: 1,
                stride: 4,
                format,
            },
        ));
        for scaled in [false, true] {
            graph.node(1).unwrap().scale = if scaled { (2., 1.) } else { (1., 1.) };
            let target = Surface {
                width: 2,
                height: 1,
                stride: 8,
                format: Format::Rgba,
            };
            let mut snapshot = Snapshot::default();
            snapshot.compile(&graph, target).unwrap();
            let mut output = [0, 0, 100, 255, 0, 0, 100, 255];
            composition::composite(&snapshot, target, &mut output, target.full(), |_| {
                Some(&[10u8, 20, 30, 0][..])
            })
            .unwrap();
            assert_eq!(&output[..4], &expected);
        }
    }
    assert_eq!(
        Format::Bgrx.convert([10, 20, 30, 0], Format::Bgra),
        [10, 20, 30, 255]
    );
    assert_eq!(
        Format::Rgbx.convert([10, 20, 30, 0], Format::Bgra),
        [30, 20, 10, 255]
    );
    assert_eq!(Format::try_from(1), Ok(Format::Bgra));
    assert_eq!(Format::try_from(2), Ok(Format::Rgba));
}
