//! Unpublished same-user local lease CLI.
//!
//! Commands: init, lease, release, recover, status. See
//! `docs/knowledge/contracts/local-lease.md`.

use oxide_storage::lease::{
    capacity_status, init_pool, lease, recover, release, render_capacity_status_human,
    render_capacity_status_json, render_status_human, render_status_json, status, LeaseError,
    LeaseRequest, Profile, BINARY_NAME, DEFAULT_STATUS_OUTPUT_BYTES,
};
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
oxide-storage-lease: same-user local lease over bounded exclusive slots

USAGE:
    oxide-storage-lease --pool <path> init --cap-bytes <n> --image-bytes <n>
    oxide-storage-lease --pool <path> lease --project <path> --workspace <path>
        --profile <dev|full-debug> --holder <id>
    oxide-storage-lease --pool <path> release --lease <id>
    oxide-storage-lease --pool <path> recover --lease <id>
    oxide-storage-lease --pool <path> status [--capacity] [--json] [--max-output-bytes <n>]

The pool directory is owner-only control state. Payload slots live on the
verified APFS backing mounted at <pool>/payload. The CLI holds an exclusive
OS lock only for the duration of the command; a finished process is not
writer completion. Cross-worktree usefulness is a dependent qualification.
";

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1).collect()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{BINARY_NAME}: {}: {error}", error.reason());
            ExitCode::from(error.exit_code())
        }
    }
}

fn run(args: Vec<OsString>) -> Result<ExitCode, LeaseError> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let mut pool = None;
    let mut command = None;
    let mut cap_bytes = None;
    let mut image_bytes = None;
    let mut project = None;
    let mut workspace = None;
    let mut profile = None;
    let mut holder = None;
    let mut lease_id = None;
    let mut json = false;
    let mut capacity = false;
    let mut max_output_bytes = DEFAULT_STATUS_OUTPUT_BYTES;
    let mut iter = args.into_iter();
    while let Some(argument) = iter.next() {
        match argument.to_str() {
            Some("--pool") => assign_path(&mut pool, &mut iter, "--pool")?,
            Some("--cap-bytes") => cap_bytes = Some(parse_u64(&mut iter, "--cap-bytes")?),
            Some("--image-bytes") => image_bytes = Some(parse_u64(&mut iter, "--image-bytes")?),
            Some("--project") => assign_path(&mut project, &mut iter, "--project")?,
            Some("--workspace") => assign_path(&mut workspace, &mut iter, "--workspace")?,
            Some("--profile") => {
                let value = next_text(&mut iter, "--profile")?;
                profile = Some(Profile::parse(&value)?);
            }
            Some("--holder") => holder = Some(next_text(&mut iter, "--holder")?),
            Some("--lease") => lease_id = Some(next_text(&mut iter, "--lease")?),
            Some("--json") => json = true,
            Some("--capacity") => capacity = true,
            Some("--max-output-bytes") => {
                max_output_bytes = parse_usize(&mut iter, "--max-output-bytes")?;
            }
            Some("init" | "lease" | "release" | "recover" | "status") => {
                if command.is_some() {
                    return Err(LeaseError::Usage("only one command is allowed"));
                }
                command = argument.to_str().map(str::to_owned);
            }
            _ => {
                return Err(LeaseError::Usage("unrecognized argument"));
            }
        }
    }
    let pool = pool.ok_or(LeaseError::Usage("--pool <path> is required"))?;
    if capacity && command.as_deref() != Some("status") {
        return Err(LeaseError::Usage("--capacity is only valid with status"));
    }
    match command.as_deref() {
        Some("init") => {
            let cap_bytes = cap_bytes.ok_or(LeaseError::Usage("--cap-bytes is required"))?;
            let image_bytes = image_bytes.ok_or(LeaseError::Usage("--image-bytes is required"))?;
            let info = init_pool(&pool, cap_bytes, image_bytes)?;
            write_json(&info)?;
            Ok(ExitCode::SUCCESS)
        }
        Some("lease") => {
            let grant = lease(LeaseRequest {
                pool,
                project: project.ok_or(LeaseError::Usage("--project is required"))?,
                workspace: workspace.ok_or(LeaseError::Usage("--workspace is required"))?,
                profile: profile.ok_or(LeaseError::Usage("--profile is required"))?,
                holder: holder.ok_or(LeaseError::Usage("--holder is required"))?,
            })?;
            write_json(&grant)?;
            Ok(ExitCode::SUCCESS)
        }
        Some("release") => {
            let id = lease_id.ok_or(LeaseError::Usage("--lease is required"))?;
            write_json(&release(&pool, &id)?)?;
            Ok(ExitCode::SUCCESS)
        }
        Some("recover") => {
            let id = lease_id.ok_or(LeaseError::Usage("--lease is required"))?;
            write_json(&recover(&pool, &id)?)?;
            Ok(ExitCode::SUCCESS)
        }
        Some("status") => {
            let rendered = if capacity {
                let report = capacity_status(&pool)?;
                if json {
                    render_capacity_status_json(&report, max_output_bytes)?
                } else {
                    render_capacity_status_human(&report, max_output_bytes)?
                }
            } else {
                let report = status(&pool)?;
                if json {
                    render_status_json(&report, max_output_bytes)?
                } else {
                    render_status_human(&report, max_output_bytes)?
                }
            };
            let stdout = std::io::stdout();
            let mut output = stdout.lock();
            output
                .write_all(&rendered.bytes)
                .map_err(|error| LeaseError::Io(error.to_string()))?;
            output
                .flush()
                .map_err(|error| LeaseError::Io(error.to_string()))?;
            if rendered.truncated || rendered.partial {
                Ok(ExitCode::from(2))
            } else {
                Ok(ExitCode::SUCCESS)
            }
        }
        _ => Err(LeaseError::Usage(
            "command must be init, lease, release, recover or status",
        )),
    }
}

fn assign_path(
    slot: &mut Option<PathBuf>,
    iter: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
) -> Result<(), LeaseError> {
    if slot.is_some() {
        return Err(LeaseError::Usage("flag may be given only once"));
    }
    let _ = flag;
    *slot = Some(PathBuf::from(
        iter.next()
            .ok_or(LeaseError::Usage("flag requires a value"))?,
    ));
    Ok(())
}

fn next_text(
    iter: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
) -> Result<String, LeaseError> {
    let _ = flag;
    iter.next()
        .ok_or(LeaseError::Usage("flag requires a value"))?
        .into_string()
        .map_err(|_| LeaseError::InvalidInput("argument must be UTF-8"))
}

fn parse_u64(
    iter: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
) -> Result<u64, LeaseError> {
    next_text(iter, flag)?
        .parse()
        .map_err(|_| LeaseError::InvalidInput("flag requires a non-negative integer"))
}

fn parse_usize(
    iter: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
) -> Result<usize, LeaseError> {
    next_text(iter, flag)?
        .parse()
        .map_err(|_| LeaseError::InvalidInput("flag requires a non-negative integer"))
}

fn write_json<T: serde::Serialize>(value: &T) -> Result<(), LeaseError> {
    let mut bytes = serde_json::to_vec(value).map_err(|error| LeaseError::Io(error.to_string()))?;
    bytes.push(b'\n');
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    output
        .write_all(&bytes)
        .map_err(|error| LeaseError::Io(error.to_string()))?;
    output
        .flush()
        .map_err(|error| LeaseError::Io(error.to_string()))
}
