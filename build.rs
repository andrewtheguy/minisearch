//! Build the frontend into Cargo's `OUT_DIR` for a release binary, where
//! `src/assets.rs` compiles it in.
//!
//! Only release builds embed the frontend. Dev builds (`cargo build`, bacon,
//! clippy, tests, rust-analyzer) skip bun entirely and the UI is served by the
//! Vite dev server with HMR. When the bundle is staged, this script sets
//! `cfg(embed_frontend)`, so the Rust side follows exactly what happened here.
//!
//! Two ways the bundle gets there: `bun run build` writing straight into
//! `OUT_DIR/frontend-dist`, or `MINISEARCH_PREBUILT_FRONTEND` naming an already
//! built directory (release CI builds it once for every target).
//! `MINISEARCH_EMBED_FRONTEND=1` embeds in a non-release build too.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};

const PREBUILT_FRONTEND: &str = "MINISEARCH_PREBUILT_FRONTEND";
const EMBED_FRONTEND: &str = "MINISEARCH_EMBED_FRONTEND";

fn main() -> Result<()> {
    println!("cargo::rustc-check-cfg=cfg(embed_frontend)");
    // Declaring any rerun directive stops Cargo from rerunning this script on
    // every change in the package, which would slow down dev rebuilds.
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed={PREBUILT_FRONTEND}");
    println!("cargo::rerun-if-env-changed={EMBED_FRONTEND}");

    let release = env::var("PROFILE").context("Cargo did not set PROFILE")? == "release";
    let forced = env::var_os(EMBED_FRONTEND).is_some_and(|v| !v.is_empty() && v != "0");
    if !release && !forced {
        return Ok(());
    }

    let root = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").context("Cargo did not set CARGO_MANIFEST_DIR")?,
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").context("Cargo did not set OUT_DIR")?)
        .join("frontend-dist");

    if let Some(prebuilt) = env::var_os(PREBUILT_FRONTEND).filter(|p| !p.is_empty()) {
        let prebuilt = PathBuf::from(prebuilt);
        let prebuilt = if prebuilt.is_absolute() {
            prebuilt
        } else {
            root.join(prebuilt)
        };
        println!("cargo::rerun-if-changed={}", prebuilt.display());
        replace_dir(&prebuilt, &output).with_context(|| {
            format!(
                "failed to stage the prebuilt frontend from {}",
                prebuilt.display()
            )
        })?;
    } else {
        build_frontend(&root, &output)?;
    }

    ensure!(
        output.join("index.html").is_file(),
        "frontend build produced no {}",
        output.join("index.html").display()
    );
    println!("cargo::rustc-cfg=embed_frontend");
    Ok(())
}

fn build_frontend(root: &Path, output: &Path) -> Result<()> {
    // Explicit inputs so neither node_modules nor build output retriggers the
    // script. Directories are walked recursively by Cargo.
    for path in [
        "frontend/src",
        "frontend/public",
        "frontend/index.html",
        "frontend/package.json",
        "frontend/bun.lock",
        "frontend/tsconfig.json",
        "frontend/vite.config.ts",
    ] {
        println!("cargo::rerun-if-changed={path}");
    }

    let frontend_dir = root.join("frontend");
    ensure!(
        frontend_dir.join("node_modules").is_dir(),
        "frontend dependencies are not installed — run `bun install --frozen-lockfile` in {}",
        frontend_dir.display()
    );
    let status = Command::new("bun")
        .args(["run", "build"])
        .current_dir(&frontend_dir)
        .env("MINISEARCH_FRONTEND_OUT_DIR", output)
        .status()
        .context("failed to run `bun run build` for the frontend — install Bun: https://bun.sh")?;
    ensure!(status.success(), "`bun run build` for the frontend failed");
    Ok(())
}

fn replace_dir(source: &Path, destination: &Path) -> Result<()> {
    ensure!(
        source.join("index.html").is_file(),
        "{} contains no index.html",
        source.display()
    );
    if destination.exists() {
        fs::remove_dir_all(destination)
            .with_context(|| format!("failed to remove {}", destination.display()))?;
    }
    copy_dir(source, destination)
}

fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read an entry in {}", source.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", entry.path().display()))?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        } else {
            bail!(
                "prebuilt frontend contains unsupported entry {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}
