//! Unpublished ordinary-Cargo qualification attempt process.

use oxide_storage::ordinary_cargo::{serve_bootstrap, OrdinaryCargoAttemptResult};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        return ExitCode::FAILURE;
    };
    if args.next().is_some() {
        return ExitCode::FAILURE;
    }
    match serve_bootstrap(Path::new(&path)) {
        Ok(OrdinaryCargoAttemptResult::Finalized(_)) => ExitCode::SUCCESS,
        Ok(OrdinaryCargoAttemptResult::Refused) => {
            eprintln!("ordinary-Cargo attempt refused before host submission");
            ExitCode::from(1)
        }
        Ok(OrdinaryCargoAttemptResult::Held(_)) => {
            eprintln!("ordinary-Cargo attempt ownership is held pending recovery");
            ExitCode::from(2)
        }
        Err(_) => {
            eprintln!("ordinary-Cargo attempt process refused or timed out");
            ExitCode::FAILURE
        }
    }
}
