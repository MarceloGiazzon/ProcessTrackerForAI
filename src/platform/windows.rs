use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
};
use windows_sys::Win32::System::Console::{
    CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, OpenJobObjectW, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED,
    GetExitCodeProcess, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SUSPEND_RESUME, ResumeThread, THREAD_SUSPEND_RESUME,
};
use windows_sys::core::BOOL;

use super::INTERRUPTS;

// Undocumented but stable since Windows XP; used by Process Explorer and
// Resource Monitor for "Suspend process". raw-dylib means no import library
// is needed at build time.
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> i32;
    fn NtResumeProcess(process: HANDLE) -> i32;
}

const STILL_ACTIVE: u32 = 259;
// Job access rights (winnt.h); windows-sys only exposes them in a much larger module.
const JOB_OBJECT_QUERY: u32 = 0x0004;
const JOB_OBJECT_TERMINATE: u32 = 0x0008;

/// The child's process tree lives in a named Job Object (`Local\ptrack-<id>`),
/// so every descendant can be stopped, paused or resumed together, and other
/// `ptrack` commands can find it by name.
pub struct Tree {
    job: Option<HANDLE>,
    pid: u32,
}

// The handle is only used from the supervisor's main thread, but the type
// must cross into it from `attach`.
unsafe impl Send for Tree {}

/// Start suspended so the child can join the job before it runs any code or
/// starts children of its own; a new process group lets us send Ctrl+Break.
pub fn prepare(cmd: &mut Command) {
    cmd.creation_flags(CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP);
}

pub fn attach(child: &Child, id: &str) -> io::Result<Tree> {
    let pid = child.id();
    let job = create_job(id);
    let assigned = match job {
        Ok(job) => {
            // SAFETY: both handles are valid for the duration of the call.
            let ok = unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) };
            if ok == 0 {
                let err = io::Error::last_os_error();
                unsafe { CloseHandle(job) };
                Err(err)
            } else {
                Ok(job)
            }
        }
        Err(e) => Err(e),
    };
    // Resume whether or not the job worked: a running, partly-controllable
    // job is better than a frozen one.
    resume_threads(pid);
    match assigned {
        Ok(job) => Ok(Tree {
            job: Some(job),
            pid,
        }),
        Err(e) => {
            eprintln!(
                "[ptrack] warning: could not put the job in a Job Object ({e}); stop will fall back to taskkill"
            );
            Ok(Tree { job: None, pid })
        }
    }
}

fn job_name(id: &str) -> Vec<u16> {
    wide(&format!("Local\\ptrack-{id}"))
}

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn create_job(id: &str) -> io::Result<HANDLE> {
    let name = job_name(id);
    // SAFETY: name is a valid null-terminated wide string.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), name.as_ptr()) };
    if job.is_null() {
        return Err(io::Error::last_os_error());
    }
    // While the supervisor lives, its death must take the tree with it, so
    // nothing is left running unrecorded. `release` lifts this after a normal exit.
    set_kill_on_close(job, true);
    Ok(job)
}

fn set_kill_on_close(job: HANDLE, on: bool) {
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    if on {
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    }
    // SAFETY: info is a properly sized, initialised struct.
    unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
    }
}

/// Resumes every thread of a process created with CREATE_SUSPENDED.
fn resume_threads(pid: u32) {
    // SAFETY: standard Toolhelp snapshot walk; handles are closed.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return;
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut ok = Thread32First(snap, &mut entry);
        while ok != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    ResumeThread(thread);
                    CloseHandle(thread);
                }
            }
            ok = Thread32Next(snap, &mut entry);
        }
        CloseHandle(snap);
    }
}

/// Asks the tree to exit with Ctrl+Break (the Windows equivalent of SIGTERM
/// for console programs). A paused tree is resumed first so it can react.
pub fn interrupt(tree: &Tree) {
    if let Some(job) = tree.job {
        for pid in job_pids(job) {
            with_process(pid, PROCESS_SUSPEND_RESUME, |h| unsafe {
                NtResumeProcess(h);
            });
        }
    }
    // SAFETY: plain call; the child shares our console and leads its own group.
    unsafe {
        GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, tree.pid);
    }
}

pub fn kill(tree: &Tree) {
    match tree.job {
        // SAFETY: valid job handle.
        Some(job) => unsafe {
            TerminateJobObject(job, 1);
        },
        None => taskkill(tree.pid),
    }
}

/// After a normal exit, background processes the command left behind (for
/// example a Gradle daemon) are allowed to outlive the supervisor.
pub fn release(tree: &Tree) {
    if let Some(job) = tree.job {
        set_kill_on_close(job, false);
    }
}

fn taskkill(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn open_job(id: &str, access: u32) -> Option<HANDLE> {
    let name = job_name(id);
    // SAFETY: name is a valid null-terminated wide string.
    let job = unsafe { OpenJobObjectW(access, 0, name.as_ptr()) };
    (!job.is_null()).then_some(job)
}

const MAX_JOB_PIDS: usize = 4096;

/// Same layout as JOBOBJECT_BASIC_PROCESS_ID_LIST, with room for many ids.
#[repr(C)]
struct PidList {
    assigned: u32,
    listed: u32,
    ids: [usize; MAX_JOB_PIDS],
}

fn job_pids(job: HANDLE) -> Vec<u32> {
    let mut list = Box::new(PidList {
        assigned: 0,
        listed: 0,
        ids: [0; MAX_JOB_PIDS],
    });
    // SAFETY: the buffer matches the API's layout and its size is passed in.
    let ok = unsafe {
        QueryInformationJobObject(
            job,
            JobObjectBasicProcessIdList,
            &mut *list as *mut PidList as *mut _,
            std::mem::size_of::<PidList>() as u32,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    let n = (list.listed as usize).min(MAX_JOB_PIDS);
    list.ids[..n].iter().map(|&p| p as u32).collect()
}

fn with_process(pid: u32, access: u32, f: impl FnOnce(HANDLE)) -> bool {
    // SAFETY: handle checked and closed.
    unsafe {
        let h = OpenProcess(access, 0, pid);
        if h.is_null() {
            return false;
        }
        f(h);
        CloseHandle(h);
    }
    true
}

pub fn kill_external(id: &str, pid: u32) -> io::Result<()> {
    match open_job(id, JOB_OBJECT_TERMINATE) {
        Some(job) => {
            // SAFETY: valid job handle, closed after use.
            unsafe {
                TerminateJobObject(job, 1);
                CloseHandle(job);
            }
            Ok(())
        }
        None => {
            if pid_alive(pid) {
                taskkill(pid);
            }
            Ok(())
        }
    }
}

pub fn suspend_external(id: &str, pid: u32) -> io::Result<()> {
    for_each_job_process(id, pid, |h| unsafe { NtSuspendProcess(h) })
}

pub fn resume_external(id: &str, pid: u32) -> io::Result<()> {
    for_each_job_process(id, pid, |h| unsafe { NtResumeProcess(h) })
}

fn for_each_job_process(id: &str, pid: u32, f: impl Fn(HANDLE) -> i32) -> io::Result<()> {
    let pids = match open_job(id, JOB_OBJECT_QUERY) {
        Some(job) => {
            let pids = job_pids(job);
            unsafe { CloseHandle(job) };
            pids
        }
        None => vec![pid],
    };
    let mut done = 0;
    for p in pids {
        if with_process(p, PROCESS_SUSPEND_RESUME, |h| {
            f(h);
        }) {
            done += 1;
        }
    }
    if done == 0 {
        return Err(io::Error::other("no process of this job could be opened"));
    }
    Ok(())
}

pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let mut alive = false;
    with_process(pid, PROCESS_QUERY_LIMITED_INFORMATION, |h| {
        let mut code = 0u32;
        // SAFETY: valid handle and out pointer.
        if unsafe { GetExitCodeProcess(h, &mut code) } != 0 {
            alive = code == STILL_ACTIVE;
        }
    });
    alive
}

unsafe extern "system" fn on_ctrl(_ctrl_type: u32) -> BOOL {
    INTERRUPTS.fetch_add(1, Ordering::SeqCst);
    1 // handled: the supervisor stops the tree and records the result itself
}

pub fn install_interrupt_handler() {
    // SAFETY: registering a handler that only touches an atomic.
    unsafe {
        SetConsoleCtrlHandler(Some(on_ctrl), 1);
    }
}

/// Starts `exe args` in the background with a hidden console that its
/// children inherit (so no console windows pop up). It tries to break away
/// from the caller's Job Object, so closing the agent does not kill the job.
pub fn spawn_detached(exe: &Path, args: &[OsString]) -> io::Result<()> {
    // Windows passes every inheritable handle to a child, including our own
    // stdout/stderr. If those are pipes (an agent capturing our output), the
    // caller would wait for the whole background job. This process is about
    // to exit, so it is safe to stop sharing them.
    for std_handle in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: GetStdHandle may return null/invalid; SetHandleInformation then just fails.
        unsafe {
            SetHandleInformation(GetStdHandle(std_handle), HANDLE_FLAG_INHERIT, 0);
        }
    }
    let base = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
    let start = |flags: u32| {
        Command::new(exe)
            .args(args)
            .creation_flags(flags)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    start(base | CREATE_BREAKAWAY_FROM_JOB)
        .or_else(|_| start(base))
        .map(|_| ())
}

/// Finds `npm` as `npm.cmd`, `gradlew` as `gradlew.bat`, etc., the way a
/// shell would, using PATH and PATHEXT. Rust's own lookup only tries `.exe`.
pub fn resolve_program(name: &str) -> PathBuf {
    let as_path = Path::new(name);
    if as_path.components().count() > 1 || as_path.is_absolute() {
        return find_with_ext(as_path).unwrap_or_else(|| as_path.to_path_buf());
    }
    let Some(path) = std::env::var_os("PATH") else {
        return as_path.to_path_buf();
    };
    for dir in std::env::split_paths(&path) {
        if let Some(found) = find_with_ext(&dir.join(name)) {
            return found;
        }
    }
    as_path.to_path_buf()
}

fn find_with_ext(base: &Path) -> Option<PathBuf> {
    if base.extension().is_some() && base.is_file() {
        return Some(base.to_path_buf());
    }
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    exts.split(';').filter(|e| !e.is_empty()).find_map(|ext| {
        let mut candidate = base.as_os_str().to_owned();
        candidate.push(ext.to_ascii_lowercase());
        let candidate = PathBuf::from(candidate);
        candidate.is_file().then_some(candidate)
    })
}

pub fn shell_command(script: &str) -> Command {
    let mut cmd = Command::new("cmd.exe");
    cmd.raw_arg(format!("/d /s /c \"{script}\""));
    cmd
}
