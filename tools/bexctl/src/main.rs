use clap::{CommandFactory, Parser};
use std::process::ExitCode;
mod args;
mod commands;
mod input;
mod output;
mod terminal;
fn main() -> ExitCode {
    let cli = args::Cli::parse();
    if let args::Command::Completions { shell } = cli.command {
        clap_complete::generate(
            shell,
            &mut args::Cli::command(),
            "bexctl",
            &mut std::io::stdout(),
        );
        return ExitCode::SUCCESS;
    }
    let Some(socket) = cli.socket else {
        args::Cli::command()
            .error(
                clap::error::ErrorKind::MissingRequiredArgument,
                "--socket PATH is required for target commands",
            )
            .exit()
    };
    let result = bexos_debug_client::UnixSocketTransport::connect(&socket)
        .map_err(|e| -> Box<dyn std::error::Error> {
            format!("connect {}: {e}", socket.display()).into()
        })
        .and_then(|t| {
            commands::run(
                &mut bexos_debug_client::DebugClient::new(t),
                cli.command,
                cli.format,
            )
        });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bexctl: {e}");
            let code = e
                .downcast_ref::<commands::RemoteExit>()
                .and_then(|r| u8::try_from(r.0).ok())
                .filter(|v| *v != 0)
                .unwrap_or(1);
            ExitCode::from(code)
        }
    }
}
