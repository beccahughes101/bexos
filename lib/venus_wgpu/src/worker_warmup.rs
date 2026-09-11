//! Exercise lazy Vello workspace allocation before advertising worker readiness.
//! The compositor continues CPU presentation throughout this initialization.
use super::{Job, Target};
use bexos_flatland::{Format, Surface};
use bexos_flatland_render::{GpuRenderer, composition::Layer, vello};
use bexos_graphics_runtime::{self as rt, Mapping};

pub(super) fn prepare(
    gpu: &crate::Device,
    renderer: &mut GpuRenderer,
    timeout: std::time::Duration,
) -> Result<Target, String> {
    let begin = rt::now_us();
    bexos_userspace::log("venus-wgpu: warming retained renderer workspace\n");
    let surface = Surface {
        width: 64,
        height: 64,
        stride: 256,
        format: Format::Rgba,
    };
    let mut target = Target::new(gpu, surface)?;
    let mut job = Job {
        layers: vec![Layer {
            scene: vello::Scene::new(),
            backdrop: None,
        }],
        output: Mapping::new(16_384).map_err(|e| format!("warmup output: {e:?}"))?,
        surface,
        damage: surface.full(),
        repair: surface.full(),
    };
    target.render(gpu, renderer, &mut job, true, timeout)?;
    if !job
        .output
        .bytes()
        .chunks_exact(4)
        .all(|p| p == [28, 18, 14, 255])
    {
        return Err("Vello warmup readback mismatch".into());
    }
    bexos_userspace::log(&format!(
        "venus-wgpu: renderer warmup complete elapsed_us={}\n",
        rt::now_us().saturating_sub(begin)
    ));
    Ok(target)
}
