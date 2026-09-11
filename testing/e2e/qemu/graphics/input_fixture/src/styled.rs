//! Exercise shared protocol clients, transaction styling/layout, and a real
//! translucent rounded backdrop while persistent input views are replaced.
use bexos_graphics::{Format, Surface, effects::Effects, style::Properties};
use bexos_graphics_runtime::{Mapping, flatland::Session};
pub fn configure(fixture: &mut crate::Fixture) {
    for (index, channel) in fixture.sessions.iter().enumerate() {
        Session::new(*channel)
            .style(
                1,
                &Properties {
                    identifier: format!("input-view-{index}"),
                    classes: "input-view".into(),
                    inline: "display:flex; width:400px; height:600px; opacity:1".into(),
                },
            )
            .expect("fixture root style");
    }
    let surface = Surface {
        width: 200,
        height: 60,
        stride: 800,
        format: Format::Bgra,
    };
    let mut pixels = Mapping::new(surface.stride as u64 * surface.height as u64).unwrap();
    for pixel in pixels.bytes_mut().chunks_exact_mut(4) {
        pixel.copy_from_slice(&[16, 32, 48, 96]);
    }
    let mut client = Session::new(fixture.sessions[0]);
    client.create(2).unwrap();
    client.attach(1, 2).unwrap();
    client.translation(2, 50, 500).unwrap();
    client.content(2, pixels.handle, surface).unwrap();
    client
        .style(
            2,
            &Properties {
                classes: "effect-panel".into(),
                inline: "width:200px; height:60px; border-radius:8px".into(),
                ..Default::default()
            },
        )
        .unwrap();
    client
        .effects(
            2,
            Effects {
                corner_radius: 8.,
                backdrop_radius: 8,
            },
        )
        .unwrap();
    fixture.buffers.push(pixels);
    bexos_userspace::log("input-fixture: styled rounded backdrop staged\n");
}
