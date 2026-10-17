//! Embeds the git commit the server was built from as `FLDR_GIT_SHA`.
//!
//! Resolution order:
//! 1. The `FLDR_GIT_SHA` environment variable, if set and non-empty. The
//!    Dockerfile forwards it from a build arg, because the Docker build
//!    context excludes `.git`.
//! 2. `git rev-parse --short=12 HEAD`, for local builds from a checkout.
//!
//! If neither works (no git binary, no `.git`) the value is left unset and
//! `/health` omits `gitSha`. The build never fails because of this.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=FLDR_GIT_SHA");

    let sha = std::env::var("FLDR_GIT_SHA")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(git_sha);

    if let Some(sha) = sha {
        println!("cargo:rustc-env=FLDR_GIT_SHA={sha}");
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!s.is_empty()).then_some(s)
}

fn git_sha() -> Option<String> {
    let sha = git(&["rev-parse", "--short=12", "HEAD"])?;

    // Rebuild when HEAD moves. `--git-path` resolves correctly inside
    // worktrees, where branch refs live in the common git dir.
    let mut watch = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    if let Some(branch) = git(&["rev-parse", "--symbolic-full-name", "HEAD"])
        && branch.starts_with("refs/")
    {
        watch.push(branch);
    }
    for p in watch {
        if let Some(path) = git(&["rev-parse", "--git-path", &p]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    Some(sha)
}
