//! Retain the first device error so a service can retire its renderer and return
//! owned buffers instead of invoking wgpu's default process-aborting handler.
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct GpuErrors(Arc<Mutex<Option<Failure>>>);

enum Failure {
    Api(wgpu::Error),
    Lost(wgpu::DeviceLostReason, String),
}

impl GpuErrors {
    pub fn install(device: &wgpu::Device) -> Self {
        let errors = Self::default();
        let callback = errors.clone();
        device.on_uncaptured_error(Arc::new(move |error| callback.record(Failure::Api(error))));
        let callback = errors.clone();
        device.set_device_lost_callback(move |reason, message| {
            callback.record(Failure::Lost(reason, message));
        });
        errors
    }

    fn record(&self, error: Failure) {
        if let Ok(mut first) = self.0.lock() {
            if first.is_none() {
                *first = Some(error);
            }
        }
    }

    /// Failure is sticky for this device epoch. A subsequent successful API
    /// call cannot make an earlier failed allocation or submission valid.
    pub fn check(&self) -> Result<(), String> {
        let first = self.0.lock().map_err(|_| "GPU error state poisoned")?;
        match first.as_ref() {
            Some(Failure::Api(error)) => Err(error.to_string()),
            Some(Failure::Lost(reason, message)) => {
                Err(format!("GPU device lost ({reason:?}): {message}"))
            }
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_failure_is_shared_sticky_and_preserved_during_error_cascades() {
        let errors = GpuErrors::default();
        assert!(errors.check().is_ok());
        let callback = errors.clone();
        std::thread::spawn(move || {
            callback.record(Failure::Api(wgpu::Error::OutOfMemory {
                source: Box::new(std::io::Error::other("allocation rejected")),
            }));
            callback.record(Failure::Api(wgpu::Error::Validation {
                source: Box::new(std::io::Error::other("invalid texture")),
                description: "secondary validation error".into(),
            }));
        })
        .join()
        .unwrap();
        assert_eq!(errors.check(), Err("Out of Memory".into()));
        assert_eq!(errors.check(), Err("Out of Memory".into()));
    }

    #[test]
    fn device_loss_is_fatal_only_for_its_device_epoch() {
        let errors = GpuErrors::default();
        errors.record(Failure::Lost(
            wgpu::DeviceLostReason::Unknown,
            "reset".into(),
        ));
        assert!(errors.check().unwrap_err().contains("reset"));
        assert!(GpuErrors::default().check().is_ok());
        assert!(errors.check().is_err());
    }
}
