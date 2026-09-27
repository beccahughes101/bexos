use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct NetworkEcho {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl NetworkEcho {
    pub fn start() -> Result<Self, String> {
        let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, 34567)).map_err(|e| e.to_string())?;
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 34567)).map_err(|e| e.to_string())?;
        tcp.set_nonblocking(true).map_err(|e| e.to_string())?;
        udp.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let worker = std::thread::spawn(move || {
            let mut streams: Vec<TcpStream> = Vec::new();
            let mut bytes = [0u8; 4096];
            while !signal.load(Ordering::Acquire) {
                loop {
                    match tcp.accept() {
                        Ok((stream, _)) => {
                            if stream.set_nonblocking(true).is_ok() {
                                streams.push(stream);
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => return,
                    }
                }
                streams.retain_mut(|stream| match stream.read(&mut bytes) {
                    Ok(0) => false,
                    Ok(length) => stream.write_all(&bytes[..length]).is_ok(),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => true,
                    Err(_) => false,
                });
                loop {
                    match udp.recv_from(&mut bytes) {
                        Ok((length, peer)) => {
                            let _ = udp.send_to(&bytes[..length], peer);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => return,
                    }
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for NetworkEcho {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
