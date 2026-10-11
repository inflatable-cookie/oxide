#![cfg(unix)]

use std::process::ExitCode;

fn main() -> ExitCode {
    // `--stream` selects the distinct private schema-v3 streaming route; no
    // argument keeps the unchanged legacy schema-v2 route. The route cannot be
    // changed by request JSON.
    let streaming = std::env::args_os().nth(1).is_some_and(|arg| arg == "--stream");
    let result = if streaming {
        oxide_storage::registered::archive_acquisition::run_streaming(
            std::io::stdin().lock(),
            std::io::stdout().lock(),
        )
    } else {
        oxide_storage::registered::archive_acquisition::run(
            std::io::stdin().lock(),
            std::io::stdout().lock(),
        )
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("registry acquisition refused: {error}");
            ExitCode::FAILURE
        }
    }
}
