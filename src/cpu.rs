//! CPU time used by a job's whole process tree, for Linux and macOS.
//!
//! The system only reports CPU time per process, and a process's numbers
//! vanish when it exits. To get one total that never goes down, each sample
//! is compared with the one before:
//! - a process seen before adds what its own time grew by;
//! - a new process adds all of its time;
//! - when a process exits, its parent's "children" time grows by the child's
//!   whole life (once the parent reaps it). Only the part not counted yet is
//!   added, so short-lived helpers that were never sampled still count, and
//!   long-lived ones are not counted twice.
//!
//! Windows does not need this: its Job Object keeps a running total itself.
//! The arithmetic is plain Rust, so it is unit-tested on every system.
//!
//! `CpuWatch` (every system) turns the total into "when did the tree last
//! use CPU", which tells a slow job from a stuck one.

use std::collections::HashMap;

/// One process of the tree, as the system reports it right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(windows, allow(dead_code))]
pub struct ProcCpu {
    pub pid: u32,
    pub ppid: u32,
    /// CPU time of the process itself (user + system), in ms.
    pub own_ms: u64,
    /// CPU time of its children that have exited and been reaped, in ms.
    pub children_ms: u64,
}

#[derive(Default)]
#[cfg_attr(windows, allow(dead_code))]
pub struct TreeCpu {
    seen: HashMap<u32, ProcCpu>,
    total_ms: u64,
}

#[cfg_attr(windows, allow(dead_code))]
impl TreeCpu {
    /// Takes the processes in the tree right now; returns the total CPU time
    /// of the tree so far, in ms.
    pub fn update(&mut self, now: Vec<ProcCpu>) -> u64 {
        let now: HashMap<u32, ProcCpu> = now.into_iter().map(|p| (p.pid, p)).collect();
        // What exited processes had already been counted for, by parent.
        let mut counted_for_gone: HashMap<u32, u64> = HashMap::new();
        for old in self.seen.values() {
            if !now.contains_key(&old.pid) {
                *counted_for_gone.entry(old.ppid).or_default() += old.own_ms + old.children_ms;
            }
        }
        for p in now.values() {
            let gained = match self.seen.get(&p.pid) {
                Some(old) => {
                    let reaped = p.children_ms.saturating_sub(old.children_ms);
                    let already = counted_for_gone.get(&p.pid).copied().unwrap_or(0);
                    p.own_ms.saturating_sub(old.own_ms) + reaped.saturating_sub(already)
                }
                None => p.own_ms + p.children_ms,
            };
            self.total_ms += gained;
        }
        self.seen = now;
        self.total_ms
    }
}

/// Decides when the tree last *used* CPU: at least 1% of one core, averaged
/// since the last time it did. Measuring against that last point (instead
/// of the previous sample) means coarse system clocks still add up, and a
/// single blip after a long idle stretch does not count.
#[derive(Default)]
pub struct CpuWatch {
    since_ms: u64,
    since_cpu_ms: u64,
    /// Last time the tree counted as busy (epoch ms).
    pub active_ms: Option<u64>,
}

impl CpuWatch {
    pub fn new(start_ms: u64) -> CpuWatch {
        CpuWatch {
            since_ms: start_ms,
            ..CpuWatch::default()
        }
    }

    /// Takes the tree's total CPU time at `now`.
    pub fn sample(&mut self, now: u64, cpu_ms: u64) {
        let used = cpu_ms.saturating_sub(self.since_cpu_ms);
        let elapsed = now.saturating_sub(self.since_ms);
        if used > 0 && used * 100 >= elapsed {
            self.mark(now, cpu_ms);
        }
    }

    /// A paused job is not stuck: its idle time starts again when it resumes.
    pub fn mark(&mut self, now: u64, cpu_ms: u64) {
        self.active_ms = Some(now);
        self.since_ms = now;
        self.since_cpu_ms = self.since_cpu_ms.max(cpu_ms);
    }
}

/// The fields of a Linux `/proc/<pid>/stat` line that matter here.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct LinuxStat {
    pub ppid: u32,
    pub pgrp: i32,
    /// utime + stime, in clock ticks.
    pub own_ticks: u64,
    /// cutime + cstime, in clock ticks.
    pub children_ticks: u64,
}

/// Parses `/proc/<pid>/stat`. The command name (in parentheses) may contain
/// spaces and parentheses itself, so fields are counted from the last `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_linux_stat(stat: &str) -> Option<LinuxStat> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let num = |i: usize| f.get(i).and_then(|v| v.parse::<u64>().ok());
    Some(LinuxStat {
        ppid: f.get(1)?.parse().ok()?,
        pgrp: f.get(2)?.parse().ok()?,
        own_ticks: num(11)? + num(12)?,
        children_ticks: num(13)? + num(14)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, own_ms: u64, children_ms: u64) -> ProcCpu {
        ProcCpu {
            pid,
            ppid,
            own_ms,
            children_ms,
        }
    }

    #[test]
    fn growth_of_known_processes_is_added() {
        let mut t = TreeCpu::default();
        assert_eq!(t.update(vec![p(10, 1, 100, 0)]), 100);
        assert_eq!(t.update(vec![p(10, 1, 250, 0), p(11, 10, 30, 0)]), 280);
    }

    #[test]
    fn a_reaped_child_is_not_counted_twice() {
        let mut t = TreeCpu::default();
        t.update(vec![p(10, 1, 100, 0), p(11, 10, 500, 0)]);
        // 11 ran 100 ms more after the last sample, exited, and 10 reaped it.
        assert_eq!(t.update(vec![p(10, 1, 100, 600)]), 700);
    }

    #[test]
    fn helpers_never_sampled_still_count() {
        let mut t = TreeCpu::default();
        t.update(vec![p(10, 1, 100, 0)]);
        // Many short commands came and went between two samples.
        assert_eq!(t.update(vec![p(10, 1, 120, 900)]), 1_020);
    }

    #[test]
    fn the_total_never_drops_when_processes_leave() {
        let mut t = TreeCpu::default();
        t.update(vec![p(10, 1, 100, 0), p(11, 10, 400, 0)]);
        // 11 left the group without being reaped by anyone we see.
        assert_eq!(t.update(vec![p(10, 1, 100, 0)]), 500);
        assert_eq!(t.update(vec![]), 500);
    }

    #[test]
    fn cpu_counts_as_used_at_one_percent_of_a_core() {
        let mut w = CpuWatch::new(0);
        // 10 ms over 2 s is 0.5%: not yet.
        w.sample(2_000, 10);
        assert_eq!(w.active_ms, None);
        // 40 ms over 4 s is 1%: busy, counted from here on.
        w.sample(4_000, 40);
        assert_eq!(w.active_ms, Some(4_000));
        // A tiny blip after a long idle stretch is not "busy".
        w.sample(304_000, 50);
        assert_eq!(w.active_ms, Some(4_000));
        // A paused job starts its idle time again.
        w.mark(310_000, 50);
        assert_eq!(w.active_ms, Some(310_000));
    }

    #[test]
    fn linux_stat_lines_are_parsed_from_the_last_parenthesis() {
        let line = "4242 (my (odd) cmd) S 4200 4242 4242 0 -1 4194560 100 0 0 0 \
                    150 50 7 3 20 0 1 0 12345 0 0";
        assert_eq!(
            parse_linux_stat(line),
            Some(LinuxStat {
                ppid: 4200,
                pgrp: 4242,
                own_ticks: 200,
                children_ticks: 10,
            })
        );
        assert_eq!(parse_linux_stat("garbage"), None);
    }
}
