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

/// Measures the CPU time of the job's process group; see `cpu.rs` for how
/// processes that come and go are counted.
#[derive(Default)]
pub struct CpuMeter {
    tree: crate::cpu::TreeCpu,
}

impl CpuMeter {
    /// Total CPU time of the tree so far, in ms. Never goes down. `None` if
    /// this system cannot tell.
    pub fn sample(&mut self, tree: &Tree) -> Option<u64> {
        let procs = group_processes(tree.pgid)?;
        Some(self.tree.update(procs))
    }
}

#[cfg(target_os = "linux")]
fn group_processes(pgid: i32) -> Option<Vec<crate::cpu::ProcCpu>> {
    // SAFETY: sysconf only reads a system setting.
    let ticks_per_s = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks_per_s <= 0 {
        return None;
    }
    let ms = |ticks: u64| ticks * 1000 / ticks_per_s as u64;
    let mut out = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse().ok()) else {
            continue;
        };
        // The process may exit while we look; then it is skipped.
        let Ok(text) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        match crate::cpu::parse_linux_stat(&text) {
            Some(st) if st.pgrp == pgid => out.push(crate::cpu::ProcCpu {
                pid,
                ppid: st.ppid,
                own_ms: ms(st.own_ticks),
                children_ms: ms(st.children_ticks),
            }),
            _ => {}
        }
    }
    Some(out)
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct MachTimebase {
    numer: u32,
    denom: u32,
}

// Declared here because libc marks its own copy deprecated (in favour of a
// crate this project does not use).
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn mach_timebase_info(info: *mut MachTimebase) -> libc::c_int;
}

#[cfg(target_os = "macos")]
fn group_processes(pgid: i32) -> Option<Vec<crate::cpu::ProcCpu>> {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};

    // CPU times come in Mach time units (nanoseconds on Intel, not on Apple silicon).
    let mut tb = MachTimebase { numer: 0, denom: 0 };
    // SAFETY: fills the struct it is given.
    unsafe { mach_timebase_info(&mut tb) };
    if tb.numer == 0 || tb.denom == 0 {
        tb = MachTimebase { numer: 1, denom: 1 };
    }
    let ms = |t: u64| (t as u128 * tb.numer as u128 / tb.denom as u128 / 1_000_000) as u64;

    // SAFETY: a null buffer only asks for the number of processes.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return None;
    }
    let mut pids: Vec<libc::pid_t> = vec![0; count as usize + 64];
    let bytes = (pids.len() * size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: the buffer is as large as we say it is.
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, bytes) };
    if count <= 0 {
        return None;
    }
    pids.truncate((count as usize).min(pids.len()));

    let mut out = Vec::new();
    for pid in pids {
        // SAFETY: plain C structs filled by the calls below; sizes are passed in.
        let mut bsd: libc::proc_bsdinfo = unsafe { zeroed() };
        let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut bsd as *mut _ as *mut c_void,
                size,
            )
        };
        if got != size || bsd.pbi_pgid as i32 != pgid {
            continue;
        }
        let mut usage: libc::rusage_info_v2 = unsafe { zeroed() };
        let ok = unsafe {
            libc::proc_pid_rusage(
                pid,
                libc::RUSAGE_INFO_V2,
                &mut usage as *mut _ as *mut libc::rusage_info_t,
            )
        };
        if ok != 0 {
            continue;
        }
        out.push(crate::cpu::ProcCpu {
            pid: pid as u32,
            ppid: bsd.pbi_ppid,
            own_ms: ms(usage.ri_user_time + usage.ri_system_time),
            children_ms: ms(usage.ri_child_user_time + usage.ri_child_system_time),
        });
    }
    Some(out)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn group_processes(_pgid: i32) -> Option<Vec<crate::cpu::ProcCpu>> {
    None
}
