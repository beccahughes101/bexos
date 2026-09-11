//! Guest fixture for bounded failure, mapping return, teardown and restart.
//! It uses private offscreen targets; display ownership remains with scened.
use crate::worker::{Completed, Job, Worker};
use bexos_flatland::{Damage, Format, Surface};
use bexos_flatland_render::{composition::Layer, vello};
use bexos_graphics_runtime::{self as rt, Mapping};
use bexos_userspace::Channel;
fn ready(worker: &Worker) -> Result<(), String> {
    let deadline = rt::now_us().saturating_add(30_000_000);
    while !worker.ready() {
        worker.poll()?;
        if worker.stopped() || rt::now_us() >= deadline {
            return Err("worker initialization failed or timed out".into());
        }
        rt::wait(&[worker.wakeup()], rt::now_us().saturating_add(1_000));
    }
    Ok(())
}
fn receive(worker: &Worker) -> Result<Completed, String> {
    let deadline = rt::now_us().saturating_add(5_000_000);
    loop {
        if let Some(frame) = worker.poll()? {
            return Ok(frame);
        }
        if rt::now_us() >= deadline {
            return Err("worker completion timeout".into());
        }
        rt::wait(&[worker.wakeup()], rt::now_us().saturating_add(1_000));
    }
}
fn stop(worker: &Worker) -> Result<(), String> {
    let begin = rt::now_us();
    worker.stop();
    // Retiring Vello's fixed workspaces unmaps hundreds of MiB under TCG.
    // This fixture budget does not change compositor or migration deadlines.
    let deadline = begin.saturating_add(30_000_000);
    while !worker.stopped() {
        worker.poll()?;
        if rt::now_us() >= deadline {
            return Err("worker teardown timeout".into());
        }
        rt::wait(&[worker.wakeup()], rt::now_us().saturating_add(1_000));
    }
    bexos_userspace::log(&format!(
        "input-fixture: worker teardown elapsed_us={} hardware_performance_verified=false\n",
        rt::now_us().saturating_sub(begin)
    ));
    Ok(())
}
pub fn verify(endpoint: Channel) -> Result<(), String> {
    let surface = Surface {
        width: 64,
        height: 64,
        stride: 256,
        format: Format::Rgba,
    };
    let partial = Damage {
        x: 16,
        y: 16,
        width: 16,
        height: 16,
    };
    for restart in 0..2 {
        let worker = Worker::start_profiled(endpoint, true)?;
        let result = (|| {
            ready(&worker)?;
            let mut output =
                Mapping::new(64 * 64 * 4).map_err(|e| format!("worker output: {e:?}"))?;
            output.bytes_mut().fill(0xa5);
            let handle = output.handle;
            let job = Job {
                layers: vec![Layer {
                    scene: vello::Scene::new(),
                    backdrop: None,
                }],
                output,
                surface,
                damage: partial,
                repair: partial,
            };
            worker
                .submit(job)
                .map_err(|_| "worker rejected ready job")?;
            let frame = receive(&worker)?;
            frame.result?;
            if frame.submission_us > frame.elapsed_us {
                return Err("submission duration exceeds worker duration".into());
            }
            let gpu_ns = frame
                .gpu_interval_ns
                .map_or_else(|| "null".into(), |v| format!("{v}"));
            bexos_userspace::log(&format!(
                "input-fixture: worker timing {{\"submission_us\":{},\"worker_us\":{},\"gpu_queue_interval_ns\":{},\"hardware_performance_verified\":false}}\n",
                frame.submission_us, frame.elapsed_us, gpu_ns
            ));
            let output = frame.job.output;
            if output.handle != handle {
                return Err("worker lost output ownership".into());
            }
            for y in 0..64 {
                for x in 0..64 {
                    let i = (y * 64 + x) * 4;
                    let expected = if (16..32).contains(&x) && (16..32).contains(&y) {
                        [28, 18, 14, 255]
                    } else {
                        [0xa5; 4]
                    };
                    if output.bytes()[i..i + 4] != expected {
                        return Err(format!("partial worker output ({x},{y})"));
                    }
                }
            }
            if restart == 0 {
                // Missing layers fail before rendering. The worker must return
                // the owned output rather than dropping an outstanding job.
                worker
                    .submit(Job {
                        layers: Vec::new(),
                        output,
                        surface,
                        damage: partial,
                        repair: partial,
                    })
                    .map_err(|_| "worker rejected failure fixture")?;
                let failed = receive(&worker)?;
                if failed.result.is_ok() || failed.job.output.handle != handle {
                    return Err("worker failure lost its output mapping".into());
                }
            }
            Ok(())
        })();
        let stopped = stop(&worker);
        drop(worker);
        result?;
        stopped?;
    }
    bexos_userspace::log(
        "input-fixture: GPU worker partial repair, failure and restart verified\n",
    );
    Ok(())
}
