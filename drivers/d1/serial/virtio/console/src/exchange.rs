use alloc::vec::Vec;
use kernel_fidl::Status;

pub trait Stream {
    fn send(&mut self, request: &[u8]) -> Result<(), Status>;
    fn progress(&mut self) -> Result<bool, Status>;
    fn receive(&mut self, output: &mut [u8]) -> Result<usize, Status>;
    fn expired(&self) -> bool;
    fn yield_once(&self);
}

pub fn exchange(stream: &mut impl Stream, frames: &[u8], count: usize) -> Result<Vec<u8>, Status> {
    bexos_rpmb_proxy::validate_frames(frames, count).map_err(|_| Status::ErrInvalidArgs)?;
    let mut request = Vec::with_capacity(frames.len() + 4);
    request.extend_from_slice(&(count as u16).to_le_bytes());
    request.extend_from_slice(&((frames.len() / 512) as u16).to_le_bytes());
    request.extend_from_slice(frames);
    stream.send(&request)?;
    let mut response = alloc::vec![0; count * 512];
    let mut received = 0;
    loop {
        let sent = stream.progress()?;
        if received < response.len() {
            let count = stream.receive(&mut response[received..])?;
            if count > response.len() - received {
                return Err(Status::ErrInvalidArgs);
            }
            received += count;
        }
        if sent && received == response.len() {
            return Ok(response);
        }
        if stream.expired() {
            return Err(Status::ErrTimedOut);
        }
        stream.yield_once();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        sent: Vec<u8>,
        remaining: usize,
        polls: usize,
        disconnected: bool,
        timeout: bool,
    }
    impl Stream for Fake {
        fn send(&mut self, bytes: &[u8]) -> Result<(), Status> {
            self.sent = bytes.to_vec();
            Ok(())
        }
        fn progress(&mut self) -> Result<bool, Status> {
            self.polls += 1;
            if self.disconnected && self.polls > 2 {
                Err(Status::ErrPeerClosed)
            } else {
                Ok(self.polls > 1)
            }
        }
        fn receive(&mut self, out: &mut [u8]) -> Result<usize, Status> {
            if self.timeout {
                return Ok(0);
            }
            let n = out.len().min(self.remaining).min(13);
            out[..n].fill(0xa5);
            self.remaining -= n;
            Ok(n)
        }
        fn expired(&self) -> bool {
            self.timeout
        }
        fn yield_once(&self) {}
    }
    fn fake() -> Fake {
        Fake {
            sent: Vec::new(),
            remaining: 512,
            polls: 0,
            disconnected: false,
            timeout: false,
        }
    }
    #[test]
    fn partial_reads_and_delayed_send_completion() {
        let mut stream = fake();
        assert_eq!(
            exchange(&mut stream, &[0; 512], 1).unwrap(),
            alloc::vec![0xa5;512]
        );
        assert_eq!(&stream.sent[..4], &[1, 0, 1, 0]);
        assert!(stream.polls > 30);
    }
    #[test]
    fn disconnect_and_timeout_do_not_return_partial_frames() {
        let mut stream = fake();
        stream.disconnected = true;
        assert_eq!(
            exchange(&mut stream, &[0; 512], 1),
            Err(Status::ErrPeerClosed)
        );
        let mut stream = fake();
        stream.timeout = true;
        assert_eq!(
            exchange(&mut stream, &[0; 512], 1),
            Err(Status::ErrTimedOut)
        );
    }
    #[test]
    fn invalid_lengths_never_reach_stream() {
        let mut stream = fake();
        assert!(exchange(&mut stream, &[0; 511], 1).is_err());
        assert!(stream.sent.is_empty());
        assert!(exchange(&mut stream, &[0; 512], 9).is_err());
        assert!(stream.sent.is_empty());
    }
}
