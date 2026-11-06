//! `tq` — command-line client for the daemon (PORTING M14).

use std::process::ExitCode;
use thermaliq_cli::{run, Env};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let code = run(
        &args,
        &Env::from_process(),
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    );
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
