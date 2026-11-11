//! argv, environment, cwd, stdout, stderr and exit-code correspondence. The
//! driver compares the exact stdout bytes, the exact stderr bytes and the
//! exit status across arms. argv[0] and absolute cwd paths are arm-specific,
//! so the program prints only the arguments after argv[0] and the cwd's own
//! leaf name.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("args={}", args.join(","));
    println!(
        "env={}",
        std::env::var("CORR_TRIAL_ENV").unwrap_or_default()
    );
    let cwd = std::env::current_dir().expect("cwd");
    println!(
        "cwd={}",
        cwd.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    );
    eprintln!("corr-trial-stderr-line");
    let code: i32 = args
        .last()
        .and_then(|a| a.parse::<i32>().ok())
        .filter(|c| *c >= 0)
        .unwrap_or(0);
    std::process::exit(code);
}
