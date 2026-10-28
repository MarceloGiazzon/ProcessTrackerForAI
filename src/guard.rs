//! `ptrack __guard -- <program> [args...]` (Unix only, internal).
//!
//! Sits between a job's supervisor and its command, as the leader of the
//! job's process group. Its stdin is a pipe whose other end only the
//! supervisor holds. If the supervisor dies, even by SIGKILL, the pipe
//! closes and the guard stops the whole group, so nothing keeps running
//! unrecorded. Windows gets the same guarantee from its Job Object.
//!
//! While the supervisor lives, the guard is invisible: it runs the command,
//! waits for it, and exits with the command's exit code.

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// How long the command gets to exit after SIGTERM before SIGKILL.
const GRACE: Duration = Duration::from_secs(5);

pub fn run(command: &[String]) -> i32 {
    let Some((program, args)) = command.split_first() else {
        eprintln!("[ptrack] guard: nothing to run");
        return 2;
    };
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[ptrack] could not start '{program}': {e}");
            return 127;
        }
    };
    // Stop requests (`ptrack stop`, Ctrl+C) are sent to the whole group and
    // are meant for the command; the guard stays to report how it ended.
    // Set only after the spawn: ignored signals are inherited by children.
    // SAFETY: setting a signal disposition to SIG_IGN is always safe.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }

    let supervisor_gone = Arc::new(AtomicBool::new(false));
    let flag = supervisor_gone.clone();
    thread::spawn(move || {
        // The supervisor never writes; end-of-file means it is gone.
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 64];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        flag.store(true, Ordering::SeqCst);
    });

    let mut stopping_since: Option<Instant> = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if supervisor_gone.load(Ordering::SeqCst) {
                    // Nobody will record this job any more: take down
                    // anything the command left behind (and the guard).
                    signal_group(libc::SIGKILL);
                }
                return exit_code(status);
            }
            Ok(None) => {}
            Err(_) => return 1,
        }
        if supervisor_gone.load(Ordering::SeqCst) {
            match stopping_since {
                None => {
                    signal_group(libc::SIGTERM);
                    // A paused job must be woken up to receive SIGTERM.
                    signal_group(libc::SIGCONT);
                    stopping_since = Some(Instant::now());
                }
                // Also ends the guard itself.
                Some(t) if t.elapsed() >= GRACE => signal_group(libc::SIGKILL),
                Some(_) => {}
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn signal_group(sig: i32) {
    // SAFETY: plain syscalls; the guard leads its own process group.
    unsafe {
        libc::killpg(libc::getpgrp(), sig);
    }
}

/// Same convention as shells: a command killed by signal N exits with 128+N.
fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}
