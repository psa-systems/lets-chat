// Emits GIT_HASH, GIT_VERSION, BUILD_DATE as compile-time env vars consumed
// via env!() in src/main.rs. Each value is read first from the build
// environment (so CI / Docker can inject it without needing git inside the
// container), then falls back to invoking `git`, then to "unknown".

use std::process::Command;

fn main() {
    let git_hash = resolve("GIT_HASH", &["rev-parse", "--short=12", "HEAD"]);
    let git_version = resolve(
        "GIT_VERSION",
        &["describe", "--tags", "--always", "--dirty"],
    );
    let build_date = std::env::var("BUILD_DATE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            run(&["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"]).unwrap_or_else(|| "unknown".to_string())
        });

    println!("cargo:rustc-env=GIT_HASH={git_hash}");
    println!("cargo:rustc-env=GIT_VERSION={git_version}");
    println!("cargo:rustc-env=BUILD_DATE={build_date}");

    println!("cargo:rerun-if-env-changed=GIT_HASH");
    println!("cargo:rerun-if-env-changed=GIT_VERSION");
    println!("cargo:rerun-if-env-changed=BUILD_DATE");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs");
    println!("cargo:rerun-if-changed=build.rs");

    // Tauri 2 build step: generates the embedded ACL/capability bundle that
    // `tauri::generate_context!()` consumes at compile time. Must run after
    // our env-var injection so cargo sees a single build script success.
    tauri_build::build();
}

fn resolve(env_key: &str, git_args: &[&str]) -> String {
    if let Ok(v) = std::env::var(env_key) {
        if !v.is_empty() {
            return v;
        }
    }
    let mut cmd = vec!["git"];
    cmd.extend(git_args);
    run(&cmd).unwrap_or_else(|| "unknown".to_string())
}

fn run(argv: &[&str]) -> Option<String> {
    let output = Command::new(argv[0]).args(&argv[1..]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
