pub struct Echo {
    pub port: u16,
    resume: std::sync::Arc<std::sync::atomic::AtomicBool>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Echo {
    pub fn resume(&self) {
        self.resume
            .store(true, std::sync::atomic::Ordering::Release);
    }
    pub fn start(paused: bool) -> Result<Self, String> {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let resume = std::sync::Arc::new(AtomicBool::new(!paused));
        let resumed = resume.clone();
        let worker = std::thread::spawn(move || {
            while !signal.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_millis(100)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut bytes = [0; 4096];
                        let mut echoed = 0;
                        while !signal.load(Ordering::Acquire) {
                            if echoed >= 32768 && !resumed.load(Ordering::Acquire) {
                                std::thread::sleep(Duration::from_millis(10));
                                continue;
                            }
                            match stream.read(&mut bytes) {
                                Ok(0) => break,
                                Ok(n) => {
                                    if stream.write_all(&bytes[..n]).is_err() {
                                        break;
                                    }
                                    echoed += n;
                                }
                                Err(e)
                                    if matches!(
                                        e.kind(),
                                        std::io::ErrorKind::WouldBlock
                                            | std::io::ErrorKind::TimedOut
                                    ) => {}
                                Err(_) => break,
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            port,
            resume,
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for Echo {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
