//! Move only bytes accepted by the destination; queued bytes survive backpressure.
use alloc::vec::Vec;
use bexos_userspace::Socket;
use smoltcp::socket::tcp;

pub(crate) fn pump(stream: Socket, socket: &mut tcp::Socket<'_>) {
    if let Ok(info) = stream.info() {
        send(
            socket,
            info.readable_bytes as usize,
            info.peer_write_closed,
            |maximum| stream.read(maximum as u32).ok(),
        );
    }
    receive(socket, |bytes| {
        stream.write(bytes).map_or(0, |written| written as usize)
    });
    if !socket.may_recv()
        && matches!(
            socket.state(),
            tcp::State::CloseWait
                | tcp::State::Closing
                | tcp::State::LastAck
                | tcp::State::TimeWait
                | tcp::State::Closed
        )
    {
        let _ = stream.shutdown(false, true);
    }
}

fn send(
    socket: &mut tcp::Socket<'_>,
    readable: usize,
    eof: bool,
    read: impl FnOnce(usize) -> Option<Vec<u8>>,
) {
    if readable == 0 {
        if eof {
            socket.close();
        }
        return;
    }
    if !socket.can_send() {
        return;
    }
    let _ = socket.send(|buffer| {
        let maximum = buffer.len().min(readable).min(4096);
        if maximum == 0 {
            return (0, ());
        }
        let Some(bytes) = read(maximum) else {
            return (0, ());
        };
        // Socket::read guarantees this bound; a malformed adapter cannot overrun it.
        if bytes.len() > maximum {
            return (0, ());
        }
        buffer[..bytes.len()].copy_from_slice(&bytes);
        (bytes.len(), ())
    });
}

fn receive(socket: &mut tcp::Socket<'_>, write: impl FnOnce(&[u8]) -> usize) {
    if !socket.can_recv() {
        return;
    }
    let _ = socket.recv(|buffer| {
        let bytes = &buffer[..buffer.len().min(4096)];
        (write(bytes).min(bytes.len()), ())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use smoltcp::time::Instant;

    fn established(rx: &[u8], tx: &[u8]) -> tcp::Socket<'static> {
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; 8]),
            tcp::SocketBuffer::new(vec![0; 8]),
        );
        let mut state = socket.checkpoint(Instant::from_millis(0));
        state.state = tcp::State::Established;
        state.rx_bytes = rx.to_vec();
        state.tx_bytes = tx.to_vec();
        socket.restore(state, Instant::from_millis(0)).unwrap();
        socket
    }

    #[test]
    fn transmit_reads_only_available_tcp_capacity_and_preserves_eof() {
        let mut socket = established(&[], b"1234567");
        send(&mut socket, 5, true, |maximum| {
            assert_eq!(maximum, 1);
            Some(b"a".to_vec())
        });
        assert_eq!(
            socket.checkpoint(Instant::from_millis(0)).tx_bytes,
            b"1234567a"
        );
        assert_eq!(socket.state(), tcp::State::Established);
        send(&mut socket, 4, true, |_| {
            panic!("full TCP queue must not consume input")
        });
        send(&mut socket, 0, true, |_| panic!("EOF must not read"));
        assert_eq!(socket.state(), tcp::State::FinWait1);
    }

    #[test]
    fn receive_keeps_bytes_on_partial_write_and_backpressure() {
        let mut socket = established(b"abcdefgh", &[]);
        receive(&mut socket, |bytes| {
            assert_eq!(bytes, b"abcdefgh");
            3
        });
        assert_eq!(socket.recv_queue(), 5);
        receive(&mut socket, |bytes| {
            assert_eq!(bytes, b"defgh");
            0
        });
        assert_eq!(socket.recv_queue(), 5);
        receive(&mut socket, |bytes| {
            assert_eq!(bytes, b"defgh");
            bytes.len()
        });
        assert_eq!(socket.recv_queue(), 0);
    }
}
