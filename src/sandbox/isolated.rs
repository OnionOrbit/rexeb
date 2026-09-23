//! Fully isolated builds via bubblewrap
//!
//! The parent serializes the resolved metadata and build options into a JSON
//! manifest, stages the extracted data next to it, then re-executes itself
//! inside `bwrap` (hidden `__sandbox-build` subcommand). The sandbox sees a
//! read-only host root, has no network access, and can only write to the
//! scratch directory the artifact is collected from.

use std::path::{Path, PathBuf};

use crate::cli::OutputFormat;
use crate::error::{RexebError, Result};
use crate::models::PackageMetadata;

/// Build manifest passed to the sandboxed child process
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SandboxManifest {
    /// Fully resolved package metadata
    pub metadata: PackageMetadata,
    /// Data directory as seen inside the sandbox
    pub data_dir: PathBuf,
    /// Output format to build
    pub format: OutputFormat,
    /// Overwrite an existing artifact in the sandbox out dir
    pub overwrite: bool,
    /// Strip ELF binaries after staging
    pub strip_binaries: bool,
    /// Original source filename (for the provenance sentinel)
    pub source_file: Option<String>,
}

/// A fully-specified isolated build request
pub struct IsolatedBuild {
    /// Fully resolved package metadata
    pub metadata: PackageMetadata,
    /// Extracted data directory on the host
    pub host_data_dir: PathBuf,
    /// Final destination of the built package on the host
    pub output_path: PathBuf,
    /// Output format to build
    pub format: OutputFormat,
    /// Overwrite `output_path` if it exists
    pub overwrite: bool,
    /// Strip ELF binaries after staging
    pub strip_binaries: bool,
    /// Original source filename (for the provenance sentinel)
    pub source_file: Option<String>,
    /// Keep the sandbox scratch dir afterwards
    pub keep_temp: bool,
}

/// Whether bubblewrap is usable on this machine
pub fn bwrap_available() -> bool {
    std::process::Command::new("bwrap")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run the package build fully isolated under bubblewrap
///
/// Layout inside the sandbox: the host scratch dir is mounted at `/tmp`
/// (which always exists, so no mountpoint creation is needed),
/// containing `manifest.json`, the staged `data/` tree, and — after the
/// child finishes — the built artifact. The host root is mounted
/// read-only and networking is disabled.
pub fn run_isolated_build(req: IsolatedBuild) -> Result<PathBuf> {
    if !bwrap_available() {
        return Err(RexebError::PackageBuild(
            "bubblewrap (bwrap) not found — install bubblewrap or use --sandbox-backend nspawn".into(),
        ));
    }
    if req.output_path.exists() && !req.overwrite {
        return Err(RexebError::PackageBuild(format!(
            "Output {} already exists (use --force to overwrite)",
            req.output_path.display()
        )));
    }
    if let Some(parent) = req.output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let scratch = tempfile::Builder::new()
        .prefix("rexeb-sandbox-")
        .tempdir()?;

    // Stage the data tree into the scratch dir (hardlinked: fast, no extra
    // disk usage) so the sandbox needs a single bind mount.
    let staged_data = scratch.path().join("data");
    clone_tree(&req.host_data_dir, &staged_data)?;

    let manifest = SandboxManifest {
        metadata: req.metadata,
        data_dir: PathBuf::from("/tmp/data"),
        format: req.format,
        overwrite: true,
        strip_binaries: req.strip_binaries,
        source_file: req.source_file,
    };
    std::fs::write(
        scratch.path().join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;

    let exe = std::env::current_exe().map_err(|e| {
        RexebError::PackageBuild(format!("Cannot re-execute inside sandbox: {}", e))
    })?;
    tracing::info!("Building isolated under bubblewrap: {}", exe.display());

    let status = std::process::Command::new("bwrap")
        .arg("--ro-bind")
        .arg("/")
        .arg("/")
        .arg("--bind")
        .arg(scratch.path())
        .arg("/tmp")
        .arg("--proc")
        .arg("/proc")
        .arg("--dev")
        .arg("/dev")
        .arg("--unshare-net")
        .arg("--die-with-parent")
        .arg("--")
        .arg(&exe)
        .arg("__sandbox-build")
        .arg("--manifest")
        .arg("/tmp/manifest.json")
        .arg("--out-dir")
        .arg("/tmp")
        .status()?;
    if !status.success() {
        return Err(RexebError::PackageBuild(format!(
            "Sandboxed build failed with status: {}",
            status
        )));
    }

    // Collect the artifact (a fresh scratch dir holds exactly one)
    let mut artifact: Option<PathBuf> = None;
    for entry in std::fs::read_dir(scratch.path())?.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or(false, |n| n.contains(".pkg.tar."))
        {
            artifact = Some(path);
            break;
        }
    }
    let artifact = artifact.ok_or_else(|| {
        RexebError::PackageBuild("Sandboxed build produced no package".into())
    })?;
    if std::fs::rename(&artifact, &req.output_path).is_err() {
        // Cross-device fallback
        std::fs::copy(&artifact, &req.output_path)?;
        let _ = std::fs::remove_file(&artifact);
    }

    if req.keep_temp {
        let kept = scratch.path().to_path_buf();
        std::mem::forget(scratch);
        tracing::info!("Kept sandbox scratch dir: {}", kept.display());
    }

    Ok(req.output_path)
}

/// Copy a tree, hardlinking regular files (same-filesystem fast path)
fn clone_tree(src_root: &Path, dst_root: &Path) -> Result<()> {
    std::fs::create_dir_all(dst_root)?;
    for entry in walkdir::WalkDir::new(src_root) {
        let entry = entry?;
        let source = entry.path();
        let rel_path = match source.strip_prefix(src_root) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if rel_path.as_os_str().is_empty() {
            continue;
        }
        let dest = dst_root.join(rel_path);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest)?;
        } else if entry.file_type().is_symlink() {
            #[cfg(unix)]
            {
                let target = std::fs::read_link(source)?;
                std::os::unix::fs::symlink(target, &dest)?;
            }
            #[cfg(not(unix))]
            let _ = std::fs::copy(source, &dest)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Hardlink first (instant, no extra space); fall back to copy
            // across devices or filesystems.
            if std::fs::hard_link(source, &dest).is_err() {
                std::fs::copy(source, &dest)?;
            }
        }
    }
    Ok(())
}
