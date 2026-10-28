//! Operating-system specific process control.
//!
//! Both back ends expose the same functions:
//! - `prepare` / `attach`: start a child so its whole process tree can be
//!   controlled later (Job Object on Windows, process group on Unix).
//! - `interrupt` / `kill` / `release`: used by the owning supervisor.
//! - `kill_external` / `suspend_external` / `resume_external`: used by other
//!   `ptrack` commands, which only know the job id and pid.
//! - `pid_alive`, `install_interrupt_handler`, `spawn_detached`, `resolve_program`,
//!   `stop_sharing_std_handles`.

use std::sync::atomic::AtomicU32;

/// Number of Ctrl+C / termination requests received by this process.
pub static INTERRUPTS: AtomicU32 = AtomicU32::new(0);

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use self::unix::*;
