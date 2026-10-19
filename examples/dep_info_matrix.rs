//! Decodes every Cargo `.fingerprint/*/dep-*` file under the given build
//! directories with the shipped decoder and prints one JSON summary. The
//! Cargo version matrix proof (oxide#091) runs it; nothing ships it.

use oxide_storage::dep_info::{decode, PathTag};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const MAX_FILES: usize = 4096;
const MAX_DEPTH: usize = 8;

fn is_dep_info(path: &Path) -> bool {
    let named = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("dep-"));
    let fingerprint = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .is_some_and(|name| name == ".fingerprint");
    named && fingerprint
}

fn collect(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) -> io::Result<()> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            collect(&path, depth + 1, found)?;
        } else if kind.is_file() && is_dep_info(&path) {
            if found.len() == MAX_FILES {
                return Err(io::Error::other("more dep-info files than the ceiling"));
            }
            found.push(path);
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let mut found = Vec::new();
    for dir in std::env::args_os().skip(1) {
        if let Err(error) = collect(Path::new(&dir), 0, &mut found) {
            eprintln!("dep-info walk failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    let (mut package, mut build, mut checksums, mut environment) = (0, 0, 0, 0);
    let mut refused = Vec::new();
    for path in &found {
        match std::fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| decode(&bytes).map_err(|refusal| format!("{refusal:?}")))
        {
            Ok(decoded) => {
                for file in &decoded.files {
                    match file.reference.tag {
                        PathTag::PackageRootRelative => package += 1,
                        PathTag::BuildRootRelative => build += 1,
                    }
                    checksums += usize::from(file.checksum.is_some());
                }
                environment += decoded.environment.len();
            }
            Err(reason) => refused.push(format!("{}: {reason}", path.display())),
        }
    }
    let summary = serde_json::json!({
        "files": found.len(),
        "decoded": found.len() - refused.len(),
        "refused": refused,
        "package_root_paths": package,
        "build_root_paths": build,
        "checksums": checksums,
        "environment_entries": environment,
    });
    println!("{summary}");
    if found.is_empty() || !refused.is_empty() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
