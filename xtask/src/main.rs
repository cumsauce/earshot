//! Build and packaging helper (`cargo xtask`), adapted from yakvc's.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};

#[derive(Debug, Parser)]
struct Args {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Build earshot-ffi with the `dist` profile and stage it for the mod.
    Natives {
        /// `host` (a quick dev build), `all` (every release target this
        /// host's OS builds), or a Rust target triple.
        #[arg(long, default_value = "host")]
        target: String,
        /// Staging directory; gets `<os>-<arch>/` subdirectories and a
        /// `natives.sha256` manifest.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Regenerate crates/earshot-ffi/include/earshot.h with cbindgen.
    Header {
        /// Fail instead of writing if the checked-in header is stale.
        #[arg(long)]
        check: bool,
    },
    /// Build the release jar into dist/.
    Dist {
        /// Natives staged by `natives --out` on each OS (CI artifacts,
        /// merged into one directory). Without it, builds them for this
        /// host only.
        #[arg(long)]
        from: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Args::parse().command {
        Cmd::Natives { target, out } => natives(
            &target,
            &out.unwrap_or_else(|| root().join("mod/build/natives")),
        ),
        Cmd::Header { check } => header(check),
        Cmd::Dist { from } => dist(from.as_deref()),
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

/// The workspace version, which the mod shares.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The oldest glibc the Linux natives run on.
const GLIBC: &str = "2.17";

fn natives(target: &str, out: &Path) -> Result<()> {
    match target {
        "host" => {
            let lib = build_ffi(None)?;
            let platform = format!("{}-{}", env::consts::OS, env::consts::ARCH);
            stage(&lib, &out.join(platform))?;
        }
        // Each OS builds its own release targets: CI runs this once per OS
        // and merges the outputs.
        "all" => match env::consts::OS {
            "linux" => {
                for triple in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
                    natives(triple, out)?;
                }
            }
            "macos" => {
                let x86 = build_ffi(Some("x86_64-apple-darwin"))?;
                let arm = build_ffi(Some("aarch64-apple-darwin"))?;
                let dir = out.join("macos-universal");
                fresh_dir(&dir)?;
                let universal = dir.join(x86.file_name().unwrap());
                run(Command::new("lipo")
                    .arg("-create")
                    .arg(&x86)
                    .arg(&arm)
                    .arg("-output")
                    .arg(&universal))?;
                println!("staged {}", universal.display());
            }
            "windows" => natives("x86_64-pc-windows-msvc", out)?,
            other => bail!("no release targets for {other}"),
        },
        triple => {
            let (os, arch) = parse_triple(triple)?;
            let lib = build_ffi(Some(triple))?;
            stage(&lib, &out.join(format!("{os}-{arch}")))?;
        }
    }
    write_manifest(out)
}

/// Builds earshot-ffi with the size-focused `dist` profile for `target` (or
/// the host) and returns the library's path. Linux targets are built with
/// cargo-zigbuild, so the library runs on any glibc since 2.17.
fn build_ffi(target: Option<&str>) -> Result<PathBuf> {
    let mut cmd = cargo(&["build", "--profile", "dist", "--package", "earshot-ffi"]);
    let mut built = root().join("target");
    let os = match target {
        None => env::consts::OS,
        Some(triple) => {
            let (os, _) = parse_triple(triple)?;
            if os == "linux" {
                cmd = cargo(&["zigbuild", "--profile", "dist", "--package", "earshot-ffi"]);
                cmd.arg(format!("--target={triple}.{GLIBC}"));
            } else {
                cmd.arg(format!("--target={triple}"));
            }
            built.push(triple);
            os
        }
    };
    run(&mut cmd)?;
    built.push("dist");
    built.push(match os {
        "linux" => "libearshot_ffi.so",
        "macos" => "libearshot_ffi.dylib",
        "windows" => "earshot_ffi.dll",
        other => bail!("unsupported OS {other}"),
    });
    Ok(built)
}

fn stage(lib: &Path, dir: &Path) -> Result<()> {
    fresh_dir(dir)?;
    let to = dir.join(lib.file_name().unwrap());
    fs::copy(lib, &to).with_context(|| format!("copying {}", lib.display()))?;
    println!("staged {}", to.display());
    Ok(())
}

/// An empty `dir`: a library from an earlier run with another name would
/// otherwise be shipped and listed in the manifest beside the new one.
fn fresh_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    fs::create_dir_all(dir)?;
    Ok(())
}

fn parse_triple(triple: &str) -> Result<(&'static str, &'static str)> {
    let arch = match triple.split('-').next() {
        Some("x86_64") => "x86_64",
        Some("aarch64") => "aarch64",
        _ => bail!("unsupported architecture in {triple}"),
    };
    let os = if triple.contains("-linux-") {
        "linux"
    } else if triple.contains("-apple-darwin") {
        "macos"
    } else if triple.contains("-windows-") {
        "windows"
    } else {
        bail!("unsupported OS in {triple}")
    };
    Ok((os, arch))
}

/// `cargo <args> --locked` on the workspace, so a Cargo.toml that no longer
/// matches Cargo.lock fails instead of quietly re-resolving.
fn cargo(args: &[&str]) -> Command {
    let mut cmd = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(root()).args(args).arg("--locked");
    cmd
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("running {:?}", cmd.get_program()))?;
    ensure!(status.success(), "{cmd:?} failed");
    Ok(())
}

/// Writes `natives.sha256` covering every staged library, in `sha256sum`
/// format with `/`-separated paths relative to `out`.
fn write_manifest(out: &Path) -> Result<()> {
    let mut entries = Vec::new();
    for platform in fs::read_dir(out)? {
        let platform = platform?;
        if !platform.file_type()?.is_dir() {
            continue;
        }
        for lib in fs::read_dir(platform.path())? {
            let lib = lib?;
            let hash: String = Sha256::digest(fs::read(lib.path())?)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let rel = format!(
                "{}/{}",
                platform.file_name().to_string_lossy(),
                lib.file_name().to_string_lossy()
            );
            entries.push((rel, hash));
        }
    }
    entries.sort();
    let mut manifest = String::new();
    for (rel, hash) in entries {
        writeln!(manifest, "{hash}  {rel}")?;
    }
    fs::write(out.join("natives.sha256"), manifest)?;
    Ok(())
}

/// Packages the release jar from the staged natives with the Gradle build,
/// which also runs the mod's unit tests.
fn dist(from: Option<&Path>) -> Result<()> {
    let natives_dir = root().join("target/dist/natives");
    if natives_dir.exists() {
        fs::remove_dir_all(&natives_dir)?;
    }
    match from {
        // Every CI artifact brings its own manifest, so copy only the
        // platform directories and write the manifest anew.
        Some(from) => {
            let platforms =
                fs::read_dir(from).with_context(|| format!("reading {}", from.display()))?;
            for platform in platforms {
                let platform = platform?;
                if !platform.file_type()?.is_dir() {
                    continue;
                }
                for lib in fs::read_dir(platform.path())? {
                    stage(&lib?.path(), &natives_dir.join(platform.file_name()))?;
                }
            }
            write_manifest(&natives_dir)?;
        }
        None => natives("host", &natives_dir)?,
    }

    let gradlew = if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    };
    run(Command::new(root().join("mod").join(gradlew))
        .current_dir(root().join("mod"))
        .arg("build")
        .arg(format!("-Pearshot.prebuiltNatives={}", natives_dir.display())))?;

    let jar = format!("earshot-{VERSION}.jar");
    let dist = root().join("dist");
    fs::create_dir_all(&dist)?;
    fs::copy(root().join("mod/build/libs").join(&jar), dist.join(&jar))?;
    println!("wrote {}", dist.join(&jar).display());
    Ok(())
}

fn header(check: bool) -> Result<()> {
    let crate_dir = root().join("crates/earshot-ffi");
    let config = cbindgen::Config::from_file(crate_dir.join("cbindgen.toml"))
        .map_err(|e| anyhow::anyhow!(e))?;
    let mut generated = Vec::new();
    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()?
        .write(&mut generated);

    let path = crate_dir.join("include/earshot.h");
    let current = fs::read(&path).unwrap_or_default();
    if current == generated {
        return Ok(());
    }
    if check {
        bail!("{} is stale; run `cargo xtask header`", path.display());
    }
    fs::write(&path, generated)?;
    println!("wrote {}", path.display());
    Ok(())
}
