use std::process::Command;

fn main() {
    // Re-run whenever HEAD or the refs (tags) change.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/");

    let version = git_describe()
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap());

    println!("cargo:rustc-env=APP_VERSION={}", version);
}

fn git_describe() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty=-modified"])
        .output()
        .ok()?;

    if output.status.success() {
        let s = String::from_utf8(output.stdout).ok()?;
        Some(s.trim().to_string())
    } else {
        None
    }
}
