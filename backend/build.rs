//! Builds the TypeScript frontend (`frontend/dist`) as part of `cargo build`/`cargo run`,
//! so a plain `cargo run` serves the app. Needs Node.js + npm.
//! Set SKIP_FRONTEND_BUILD=1 to skip (e.g. when only working on the backend).

use std::path::Path;
use std::process::Command;

fn npm(args: &[&str], dir: &Path) -> bool {
    let program = if cfg!(windows) { "npm.cmd" } else { "npm" };
    match Command::new(program).args(args).current_dir(dir).status() {
        Ok(s) => s.success(),
        Err(_) => false,
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=SKIP_FRONTEND_BUILD");
    let frontend = Path::new(env!("CARGO_MANIFEST_DIR")).join("../frontend");
    for p in [
        "src",
        "public",
        "index.html",
        "package.json",
        "package-lock.json",
        "tsconfig.json",
        "vite.config.ts",
    ] {
        println!("cargo:rerun-if-changed={}", frontend.join(p).display());
    }
    // The embedding macro needs the folder to exist even if the build is skipped or fails.
    let _ = std::fs::create_dir_all(frontend.join("dist"));
    if std::env::var_os("SKIP_FRONTEND_BUILD").is_some() {
        return;
    }
    if !frontend.join("node_modules").exists() && !npm(&["install"], &frontend) {
        println!("cargo:warning=could not run `npm install` in frontend/ (is Node.js installed?). The web app will not be served.");
        return;
    }
    if !npm(&["run", "build"], &frontend) {
        println!("cargo:warning=frontend build failed; the web app will not be served (fix the error above or set SKIP_FRONTEND_BUILD=1).");
    }
}
