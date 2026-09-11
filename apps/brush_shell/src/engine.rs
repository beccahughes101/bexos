//! Brush evaluator integration. The provider owns all terminal streams.
use brush_builtins::{BuiltinSet, ShellBuilderExt};
use brush_core::{
    Shell,
    openfiles::{OpenFile, Stream},
};
use std::{
    collections::{HashMap, VecDeque},
    io::{self, Read, Write},
    sync::{Arc, Mutex},
};

pub type Brush = Shell<brush_core::extensions::DefaultShellExtensions>;
pub const BUFFER_LIMIT: usize = 32768;
#[derive(Clone, Default)]
pub struct Buffer(
    pub Arc<Mutex<VecDeque<u8>>>,
    pub Arc<std::sync::atomic::AtomicBool>,
);
impl Buffer {
    pub fn append(&self, bytes: &[u8]) -> io::Result<()> {
        let mut b = self.0.lock().unwrap();
        if b.len() + bytes.len() > BUFFER_LIMIT {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        b.extend(bytes);
        Ok(())
    }
}
impl Read for Buffer {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut b = self.0.lock().unwrap();
        if b.is_empty() && !out.is_empty() && !self.1.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let n = out.len().min(b.len());
        for dest in &mut out[..n] {
            *dest = b.pop_front().unwrap();
        }
        Ok(n)
    }
}
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut b = self.0.lock().unwrap();
        let n = bytes.len().min(BUFFER_LIMIT - b.len());
        if n == 0 && !bytes.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        b.extend(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Stream for Buffer {
    fn clone_box(&self) -> Box<dyn Stream> {
        Box::new(self.clone())
    }
}
pub fn attach(shell: &mut Brush, streams: &[Buffer; 3]) {
    shell.open_files_mut().update_from(
        streams
            .iter()
            .enumerate()
            .map(|(i, b)| (i as i32, OpenFile::Stream(Box::new(b.clone())))),
    );
}
pub async fn create(
    mut environment: Vec<(String, String)>,
    streams: [Buffer; 3],
) -> Result<Brush, String> {
    let fds: HashMap<_, _> = streams
        .into_iter()
        .enumerate()
        .map(|(i, b)| (i as i32, OpenFile::Stream(Box::new(b))))
        .collect();
    let mut builder = Shell::builder()
        .default_builtins(BuiltinSet::BashMode)
        .working_dir("/data".into())
        .shell_name("brush".into())
        .profile(brush_core::ProfileLoadBehavior::Skip)
        .rc(brush_core::RcLoadBehavior::Skip)
        .do_not_inherit_env(true)
        .interactive(true)
        .fds(fds);
    for (name, value) in [("HOME", "/data"), ("PATH", "/pkg/bin:/system/bin")] {
        if !environment.iter().any(|(key, _)| key == name) {
            environment.push((name.into(), value.into()));
        }
    }
    for (name, value) in environment {
        let mut variable = brush_core::ShellVariable::new(value);
        variable.export();
        builder = builder.var(name, variable);
    }
    builder.build().await.map_err(|e| e.to_string())
}
