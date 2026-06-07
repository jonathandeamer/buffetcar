//! Build script: set `GIT_DESCRIBE` so the version module can format a
//! human-readable version line that includes commit distance and dirty state.

use std::process::Command;

fn main() {
    // Re-run when the HEAD ref or packed-refs change (branch switch, commit,
    // pull). We deliberately avoid re-running on every file change.
    println!("cargo::rerun-if-changed=.git/HEAD");
    println!("cargo::rerun-if-changed=.git/refs");

    if let Some(describe) = git_describe() {
        println!("cargo::rustc-env=GIT_DESCRIBE={describe}");
    }
    // If git is unavailable (tarball / crates.io), GIT_DESCRIBE stays unset
    // and `option_env!("GIT_DESCRIBE")` returns None at compile time.
}

fn git_describe() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--dirty", "--always"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let desc = String::from_utf8(output.stdout).ok()?;
    Some(desc.trim().to_string())
}
