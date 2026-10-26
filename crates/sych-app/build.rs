// The reviewer build identity: results name it, and a self-review server checks it against
// its trusted pin. `SYCH_BUILD_COMMIT` overrides it in builds without a `.git` directory.
use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=SYCH_BUILD_COMMIT");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let commit = env::var("SYCH_BUILD_COMMIT").ok().or_else(|| {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    });
    println!(
        "cargo:rustc-env=SYCH_BUILD_COMMIT={}",
        commit.unwrap_or_else(|| "unknown".into())
    );
}
