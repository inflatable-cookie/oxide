//! Unpublished private storage-runtime entry point, enabled only by private-qualification.
//! Exit 0 is an authenticated shutdown. Exit 2 is lifetime or control loss, which
//! preserves durable ownership state and never proves writer completion.
fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        return std::process::ExitCode::FAILURE;
    };
    if args.next().is_some() {
        return std::process::ExitCode::FAILURE;
    }
    match oxide_storage::registered::storage_runtime::serve_bootstrap(std::path::Path::new(&path)) {
        Ok(oxide_storage::registered::storage_runtime::Stop::Shutdown(_)) => {
            std::process::ExitCode::SUCCESS
        }
        Ok(stop) => {
            eprintln!("private storage runtime stopped: {stop:?}");
            std::process::ExitCode::from(2)
        }
        Err(_) => {
            eprintln!("private storage runtime refused or held its state");
            std::process::ExitCode::FAILURE
        }
    }
}
