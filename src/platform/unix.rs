use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;

use super::INTERRUPTS;

/// The child runs as the leader of its own process group; the group id is its pid.
pub struct Tree {
    pgid: i32,
}

/// The supervisor keeps this open for as long as it lives; see `guard.rs`.
pub type Lifeline = io::PipeWriter;

/// Wraps the command in `ptrack __guard`, which leads a new process group
/// and stops the whole group if the supervisor ever dies without cleaning
/// up. The guard's stdin is the lifeline pipe; the command gets /dev/null.
pub fn prepare(cmd: Command) -> io::Result<(Command, Lifeline)> {
    let (reader, writer) = io::pipe()?;
    let mut guard = Command::new(std::env::current_exe()?);
    guard
        .arg("__guard")
        .arg("--")
        .arg(cmd.get_program())
        .args(cmd.get_args());
    if let Some(dir) = cmd.get_current_dir() {
        guard.current_dir(dir);
    }
    for (key, value) in cmd.get_envs() {
        match value {
            Some(v) => guard.env(key, v),
            None => guard.env_remove(key),
        };
    }
    guard.stdin(reader).process_group(0);
    Ok((guard, writer))
}

pub fn attach(child: &Child, _id: &str) -> io::Result<Tree> {
    Ok(Tree {
        pgid: child.id() as i32,
    })
}

/// Asks the whole tree to exit (SIGTERM). SIGCONT follows so a paused tree
/// can actually receive it.
pub fn interrupt(tree: &Tree) {
    signal_group(tree.pgid, libc::SIGTERM);
    signal_group(tree.pgid, libc::SIGCONT);
}

pub fn kill(tree: &Tree) {
    signal_group(tree.pgid, libc::SIGKILL);
}

/// Nothing to undo on Unix: leftover background processes keep running, as
/// they would after any shell command.
pub fn release(_tree: &Tree) {}

pub fn kill_external(_id: &str, pid: u32) -> io::Result<()> {
    signal_group_checked(pid as i32, libc::SIGKILL)
}

pub fn suspend_external(_id: &str, pid: u32) -> io::Result<()> {
    signal_group_checked(pid as i32, libc::SIGSTOP)
}

pub fn resume_external(_id: &str, pid: u32) -> io::Result<()> {
    signal_group_checked(pid as i32, libc::SIGCONT)
}

fn signal_group(pgid: i32, sig: i32) {
    let _ = signal_group_checked(pgid, sig);
}

fn signal_group_checked(pgid: i32, sig: i32) -> io::Result<()> {
    if pgid <= 1 {
        return Err(io::Error::other(
            "refusing to signal an invalid process group",
        ));
    }
    // SAFETY: plain syscall with integer arguments.
    if unsafe { libc::killpg(pgid, sig) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: signal 0 only checks for existence and permission.
    if unsafe { libc::kill(pid as i32, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

extern "C" fn on_signal(_sig: libc::c_int) {
    INTERRUPTS.fetch_add(1, Ordering::SeqCst);
}

pub fn install_interrupt_handler() {
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    // SAFETY: the handler only touches an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGHUP, handler);
    }
}

/// Starts `exe args` in a new session, detached from this terminal.
pub fn spawn_detached(exe: &Path, args: &[OsString]) -> io::Result<()> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and allowed between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn().map(|_| ())
}

pub fn resolve_program(name: &str) -> PathBuf {
    PathBuf::from(name)
}

pub fn shell_command(script: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    cmd
}

/// Nothing to do on Unix: children get /dev/null for stdio when asked, and
/// Rust opens every other file descriptor as close-on-exec.
pub fn stop_sharing_std_handles() {}
