//! Sandbox environment for package testing and building

mod nspawn;

pub use nspawn::NspawnSandbox;

use std::path::Path;
use crate::error::Result;

/// Trait for sandbox implementations
pub trait Sandbox {
    /// Initialize the sandbox
    fn init(&mut self) -> Result<()>;

    /// Run a command inside the sandbox
    fn run_command(&self, command: &str, args: &[&str]) -> Result<i32>;

    /// Copy a file into the sandbox
    fn copy_in(&self, src: &Path, dest: &Path) -> Result<()>;

    /// Copy a file out of the sandbox
    fn copy_out(&self, src: &Path, dest: &Path) -> Result<()>;

    /// Clean up sandbox resources
    fn cleanup(&mut self) -> Result<()>;
}

/// Create a new default sandbox
pub fn create_sandbox(root_dir: &Path) -> Result<Box<dyn Sandbox>> {
    // For now, we only support systemd-nspawn
    Ok(Box::new(NspawnSandbox::new(root_dir)?))
}