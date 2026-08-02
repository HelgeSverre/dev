#![allow(unsafe_code)]

use std::io;
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt as _;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Console::{
    GenerateConsoleCtrlEvent, GetConsoleMode, GetStdHandle, SetConsoleCtrlHandler, SetConsoleMode,
    CTRL_BREAK_EVENT, CTRL_C_EVENT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_ERROR_HANDLE,
    STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

static ACTIVE_CHILD_GROUP: AtomicU32 = AtomicU32::new(0);
static CONTROL_FORWARDER_INSTALLED: AtomicBool = AtomicBool::new(false);

pub(super) fn execute_command(mut command: Command) -> io::Result<i32> {
    enable_virtual_terminal_processing();
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);

    let forwarder = ControlForwarder::install()?;
    let job = Job::kill_on_close()?;
    let mut child = command.spawn()?;
    if let Err(error) = job.assign(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    forwarder.activate(child.id());
    let status = child.wait()?;
    Ok(status.code().unwrap_or(1))
}

struct Job(HANDLE);

impl Job {
    fn kill_on_close() -> io::Result<Self> {
        // SAFETY: Both optional pointer arguments are null, requesting default
        // security and an unnamed job. The returned handle is owned by `Job`.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(handle);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .expect("Windows job information structure size fits in u32");
        // SAFETY: `job.0` is a live Job Object handle and the pointer and size
        // describe `limits` for the requested information class.
        let configured = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                size,
            )
        };
        if configured == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: Both handles are live for the duration of the call. `Child`
        // retains ownership of its process handle and `Job` retains its handle.
        let assigned = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) };
        if assigned == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: `Job` owns this non-null handle and closes it exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct ControlForwarder;

impl ControlForwarder {
    fn install() -> io::Result<Self> {
        CONTROL_FORWARDER_INSTALLED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| io::Error::other("another child process group is already active"))?;
        // SAFETY: `console_control_handler` has the required system ABI and is
        // static for the remainder of the process lifetime.
        let installed = unsafe { SetConsoleCtrlHandler(Some(console_control_handler), 1) };
        if installed == 0 {
            CONTROL_FORWARDER_INSTALLED.store(false, Ordering::Release);
            return Err(io::Error::last_os_error());
        }
        Ok(Self)
    }

    fn activate(&self, child_group: u32) {
        ACTIVE_CHILD_GROUP.store(child_group, Ordering::Release);
    }
}

impl Drop for ControlForwarder {
    fn drop(&mut self) {
        ACTIVE_CHILD_GROUP.store(0, Ordering::Release);
        // SAFETY: This removes the same static handler installed by `install`.
        let _ = unsafe { SetConsoleCtrlHandler(Some(console_control_handler), 0) };
        CONTROL_FORWARDER_INSTALLED.store(false, Ordering::Release);
    }
}

unsafe extern "system" fn console_control_handler(event: u32) -> i32 {
    if !matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
        return 0;
    }
    let child_group = ACTIVE_CHILD_GROUP.load(Ordering::Acquire);
    if child_group == 0 {
        // Swallow interrupts during the narrow spawn-and-assign window so the
        // launcher cannot exit before the child is contained by the Job Object.
        return i32::from(CONTROL_FORWARDER_INSTALLED.load(Ordering::Acquire));
    }
    // Windows cannot target CTRL_C_EVENT to one process group. A targeted
    // CTRL_BREAK_EVENT is the supported graceful-interrupt equivalent for a
    // child created with CREATE_NEW_PROCESS_GROUP.
    // SAFETY: `child_group` is the process-group identifier returned by spawn.
    unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child_group) }
}

fn enable_virtual_terminal_processing() {
    for stream in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: The standard-handle identifier is one of the documented
        // constants. Redirected or missing handles are handled by API failure.
        let handle = unsafe { GetStdHandle(stream) };
        if handle.is_null() {
            continue;
        }
        let mut mode = 0;
        // SAFETY: `mode` is valid writable storage for the duration of the call.
        if unsafe { GetConsoleMode(handle, &raw mut mode) } == 0 {
            continue;
        }
        // SAFETY: `handle` was accepted by GetConsoleMode and the new value
        // only adds the documented virtual-terminal output flag.
        let _ = unsafe { SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) };
    }
}
