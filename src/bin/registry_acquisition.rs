#![cfg(unix)]

use std::process::ExitCode;

fn main() -> ExitCode {
    match oxide_storage::registered::archive_acquisition::run(
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("registry acquisition refused: {error}");
            ExitCode::FAILURE
        }
    }
}
