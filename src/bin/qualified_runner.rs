//! Unpublished one-attempt entry point, enabled only by private-qualification.
fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        return std::process::ExitCode::FAILURE;
    };
    if args.next().is_some() {
        return std::process::ExitCode::FAILURE;
    }
    match oxide_storage::registered::qualification::serve_bootstrap(std::path::Path::new(&path)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("private qualification runner refused or held the attempt");
            std::process::ExitCode::FAILURE
        }
    }
}
