use super::{map_kernel_error, migration::Runtime, reply_frame};
use crate::{
    pending::MAX_PENDING,
    server::{rx_complete, rx_error, tx_complete},
};
use bexos_userspace::live_migration::Source;
use ethernet_fidl::{FidlDecode, FrameEntry, FrameOpcode, Status};

pub(super) fn poll(state: &mut Runtime, source: &mut Source) {
    for fifo in &state.fifos {
        let Ok(message) = fifo.try_recv() else {
            continue;
        };
        let Ok(entry) = FrameEntry::decode(&message.bytes, &[]) else {
            continue;
        };
        match entry.opcode {
            FrameOpcode::RxSupply if state.pending.rx.len() < MAX_PENDING => {
                state.pending.rx.push_back((*fifo, entry))
            }
            FrameOpcode::TxSend if state.pending.tx.len() < MAX_PENDING => {
                state.pending.tx.push_back((*fifo, entry))
            }
            FrameOpcode::RxSupply => reply_frame(*fifo, rx_error(entry)),
            FrameOpcode::TxSend => reply_frame(*fifo, tx_complete(entry, Status::ErrNoMemory)),
            _ => {}
        }
        source.changed(0);
    }
    let hardware = state.hardware.as_mut().unwrap();
    if let Some((fifo, entry)) = state.pending.transmitting {
        match hardware.poll_transmit() {
            Ok(false) => {}
            result => {
                state.pending.transmitting = None;
                let status = result.map_err(map_kernel_error).err().unwrap_or(Status::Ok);
                reply_frame(fifo, tx_complete(entry, status));
                source.changed_keys([0, 2, 3]);
            }
        }
    }
    if state.pending.transmitting.is_none() {
        if let Some((fifo, entry)) = state.pending.tx.pop_front() {
            let result = state
                .server
                .as_mut()
                .unwrap()
                .frame_slice(entry)
                .and_then(|bytes| hardware.begin_transmit(bytes).map_err(map_kernel_error));
            match result {
                Ok(()) => state.pending.transmitting = Some((fifo, entry)),
                Err(status) => reply_frame(fifo, tx_complete(entry, status)),
            }
            source.changed_keys([0, 2, 3]);
        }
    }
    if let Some(&(fifo, entry)) = state.pending.rx.front() {
        let result = state
            .server
            .as_mut()
            .unwrap()
            .frame_slice(entry)
            .and_then(|bytes| hardware.try_receive(bytes).map_err(map_kernel_error));
        match result {
            Ok(None) => {}
            result => {
                state.pending.rx.pop_front();
                reply_frame(
                    fifo,
                    match result {
                        Ok(Some(length)) => rx_complete(entry, length),
                        _ => rx_error(entry),
                    },
                );
                source.changed(0);
            }
        }
        // Posting a new RX descriptor changes the hardware snapshot even if
        // the device has not completed a packet yet.
        source.changed_keys([2, 3]);
    }
}
