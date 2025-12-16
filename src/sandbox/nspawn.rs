//! Systemd-nspawn sandbox implementation

use std::path::{Path, PathBuf};
use std::process::Command;
use crate::error::{RexebError, Result};
use super::Sandbox;

/// Sandbox using systemd-nspawn
pub struct NspawnSandbox {
    root_dir: PathBuf,
}

impl NspawnSandbox {
    /// Create a new nspawn sandbox
    pub fn new(root_dir: &Path) -> Result<Self> {
        Ok(Self {
            root_dir: root_dir.to_path_buf(),
        })
    }

    /// Check if systemd-nspawn is available
    fn check_availability() -> Result<()> {
        let status = Command::new("systemd-nspawn")
            .arg("--version")
            .output()
            .map_err(|_| RexebError::Other("systemd-nspawn not found".into()))?;
        
        if !status.status.success() {
            return Err(RexebError::Other("systemd-nspawn check failed".into()));
        }
        Ok(())
    }
}

impl Sandbox for NspawnSandbox {
    fn init(&mut self) -> Result<()> {
        Self::check_availability()?;
        
        // Ensure root directory exists
        if !self.root_dir.exists() {
            std::fs::create_dir_all(&self.root_dir)?;
        }

        // Initialize a minimal Arch system if empty
        // In a real scenario, we might want to bootstrap pacstrap here
        // For now, we assume the user provides a rootfs or we use a temporary one
        
        Ok(())
    }

    fn run_command(&self, command: &str, args: &[&str]) -> Result<i32> {
        let mut cmd = Command::new("sudo");
        cmd.arg("systemd-nspawn")
           .arg("-D")
           .arg(&self.root_dir)
           .arg("--as-pid2") // Run as PID 2 (init is PID 1)
           .arg(command);
        
        for arg in args {
            cmd.arg(arg);
        }

        let status = cmd.status()?;
        
        Ok(status.code().unwrap_or(-1))
    }

    fn copy_in(&self, src: &Path, dest: &Path) -> Result<()> {
        let target = self.root_dir.join(dest.strip_prefix("/").unwrap_or(dest));
        
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        if src.is_dir() {
            // Recursive copy
            // For simplicity using cp command
            Command::new("cp")
                .arg("-r")
                .arg(src)
                .arg(&target)
                .status()?;
        } else {
            std::fs::copy(src, &target)?;
        }
        
        Ok(())
    }

    fn copy_out(&self, src: &Path, dest: &Path) -> Result<()> {
        let source = self.root_dir.join(src.strip_prefix("/").unwrap_or(src));
        
        if !source.exists() {
            return Err(RexebError::file_not_found(&source));
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if source.is_dir() {
            Command::new("cp")
                .arg("-r")
                .arg(&source)
                .arg(dest)
                .status()?;
        } else {
            std::fs::copy(&source, dest)?;
        }

        Ok(())
    }

    fn cleanup(&mut self) -> Result<()> {
        // Cleanup typically handled by systemd-nspawn ephemeral mode (-x) 
        // but if we created a persistent root, we might want to keep it or delete it based on config
        Ok(())
    }
}