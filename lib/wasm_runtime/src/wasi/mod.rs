//! WASI 0.2 bindings. Host implementations resolve only granted resources.
wasmtime::component::bindgen!({
    path: "external/+_repo_rules+wasmtime_wasi_wit/src/p2/wit",
    world: "wasi:cli/command",
    imports: {
        "wasi:io/poll.poll": async | trappable,
        "wasi:io/poll.[method]pollable.block": async | trappable,
        "wasi:io/streams.[method]input-stream.blocking-read": async | trappable,
        "wasi:io/streams.[method]input-stream.blocking-skip": async | trappable,
        "wasi:io/streams.[method]output-stream.blocking-write-and-flush": async | trappable,
        "wasi:io/streams.[method]output-stream.blocking-flush": async | trappable,
        "wasi:io/streams.[method]output-stream.blocking-write-zeroes-and-flush": async | trappable,
        "wasi:io/streams.[method]output-stream.blocking-splice": async | trappable,
        default: trappable,
    },
    with: {
        "wasi:sockets/network.network": crate::wasi::sockets::Network,
        "wasi:sockets/tcp.tcp-socket": crate::wasi::sockets::Tcp,
        "wasi:sockets/udp.udp-socket": crate::wasi::sockets::Udp,
        "wasi:sockets/udp.incoming-datagram-stream": crate::wasi::sockets::Incoming,
        "wasi:sockets/udp.outgoing-datagram-stream": crate::wasi::sockets::Outgoing,
        "wasi:sockets/ip-name-lookup.resolve-address-stream": crate::wasi::sockets::Addresses,
        "wasi:filesystem/types.descriptor": crate::wasi::state::Descriptor,
        "wasi:filesystem/types.directory-entry-stream": crate::wasi::state::DirectoryStream,
        "wasi:io/streams.input-stream": crate::wasi::state::Input,
        "wasi:io/streams.output-stream": crate::wasi::state::Output,
        "wasi:io/poll.pollable": crate::wasi::state::Pollable,
        "wasi:io/error.error": crate::wasi::state::IoError,
        "wasi:cli/terminal-input.terminal-input": crate::wasi::state::TerminalInput,
        "wasi:cli/terminal-output.terminal-output": crate::wasi::state::TerminalOutput,
    },
    exports: { default: async },
});

pub(crate) mod cli;
mod clocks;
mod io;
pub mod state;

pub mod filesystem;

pub mod sockets;

pub mod checkpoint;

mod output;
pub mod output_state;
