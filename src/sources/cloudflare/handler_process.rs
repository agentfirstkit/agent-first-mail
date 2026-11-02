//! Own the handler's ordinary process tree until it exits or is cancelled.
use std::io;
use std::process::{Child, Command, ExitStatus};

pub(super) struct HandlerProcess {
    child: Child,
    finished: bool,
    #[cfg(windows)]
    job: windows_job::Job,
}

impl HandlerProcess {
    pub(super) fn spawn(command: &mut Command) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        let job = {
            use std::os::windows::process::CommandExt;
            use windows::Win32::System::Threading::CREATE_SUSPENDED;
            command.creation_flags(CREATE_SUSPENDED.0);
            windows_job::Job::new()?
        };
        let child = command.spawn()?;
        #[cfg(windows)]
        let child = {
            let mut child = child;
            // The child cannot create descendants before it belongs to our job.
            if let Err(error) = job.attach_and_resume(&child) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            child
        };
        Ok(Self {
            child,
            finished: false,
            #[cfg(windows)]
            job,
        })
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if crate::sources::consumer_shutdown::requested() {
            self.stop()?;
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        #[cfg(unix)]
        {
            if !self.exited_without_reaping()? {
                return Ok(None);
            }
            self.stop().map(Some)
        }
        #[cfg(not(unix))]
        {
            let status = self.child.try_wait()?;
            if status.is_some() {
                #[cfg(windows)]
                self.job.terminate()?;
                self.finished = true;
            }
            Ok(status)
        }
    }

    #[cfg(unix)]
    fn exited_without_reaping(&self) -> io::Result<bool> {
        // Reserve the leader's PID/PGID until its group has been stopped.
        // SAFETY: siginfo_t is a plain C output record; waitid writes it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info.si_signo != 0)
    }

    #[cfg(target_os = "macos")]
    fn no_live_group_members(&self, group: libc::pid_t) -> io::Result<bool> {
        if !self.exited_without_reaping()? {
            return Ok(false);
        }
        // Darwin can return EPERM for an all-zombie group. Verify every member
        // instead of ignoring a permission failure while a child still runs.
        // proc_listpgrppids returns PID count, not bytes, and includes zombies.
        let mut members = vec![0 as libc::pid_t; 16];
        loop {
            let bytes = i32::try_from(std::mem::size_of_val(members.as_slice()))
                .map_err(io::Error::other)?;
            let count = unsafe {
                *libc::__error() = 0;
                libc::proc_listpgrppids(group, members.as_mut_ptr().cast(), bytes)
            };
            let error = io::Error::last_os_error();
            if count < 0 || (count == 0 && error.raw_os_error() != Some(0)) {
                return Err(error);
            }
            if count as usize == members.len() {
                let size = members
                    .len()
                    .checked_mul(2)
                    .ok_or_else(|| io::Error::other("handler process group is too large"))?;
                members.resize(size, 0);
                continue;
            }
            members.truncate(count as usize);
            break;
        }
        for pid in members.into_iter().filter(|pid| *pid != 0 && *pid != group) {
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of_val(&info) as i32;
            let read = unsafe {
                *libc::__error() = 0;
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    (&mut info as *mut libc::proc_bsdinfo).cast(),
                    size,
                )
            };
            if read != size {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    continue;
                }
                return Err(io::Error::other(format!(
                    "inspect handler group member: {error}"
                )));
            }
            if info.pbi_pgid == group as u32 && info.pbi_status != libc::SZOMB {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn stop(&mut self) -> io::Result<ExitStatus> {
        #[cfg(unix)]
        {
            let group = i32::try_from(self.child.id()).map_err(io::Error::other)?;
            // SAFETY: spawn put this child in its own process group. Its PID
            // cannot be reused while the direct child remains unreaped.
            if unsafe { libc::killpg(group, libc::SIGKILL) } != 0 {
                let error = io::Error::last_os_error();
                #[cfg(target_os = "macos")]
                let empty_group = error.raw_os_error() == Some(libc::EPERM)
                    && self.no_live_group_members(group)?;
                #[cfg(not(target_os = "macos"))]
                let empty_group = false;
                if error.raw_os_error() != Some(libc::ESRCH) && !empty_group {
                    return Err(error);
                }
            }
        }
        #[cfg(windows)]
        self.job.terminate()?;
        #[cfg(not(any(unix, windows)))]
        self.child.kill()?;
        let status = self.child.wait()?;
        self.finished = true;
        Ok(status)
    }
}

impl Drop for HandlerProcess {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.stop();
        }
    }
}

#[cfg(windows)]
mod windows_job {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::process::Child;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
    };
    use windows::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    pub(super) struct Job(OwnedHandle);

    // SAFETY at each call: every OwnedHandle below takes sole ownership of a
    // successful Win32 handle; borrowed HANDLEs never outlive their owner.
    impl Job {
        pub(super) fn new() -> io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(None, None) }.map_err(io::Error::other)?;
            Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle.0) }))
        }

        pub(super) fn attach_and_resume(&self, child: &Child) -> io::Result<()> {
            unsafe {
                AssignProcessToJobObject(
                    HANDLE(self.0.as_raw_handle()),
                    HANDLE(child.as_raw_handle()),
                )
            }
            .map_err(io::Error::other)?;
            let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }
                .map_err(io::Error::other)?;
            let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.0) };
            let snapshot_handle = HANDLE(snapshot.as_raw_handle());
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            unsafe { Thread32First(snapshot_handle, &mut entry) }.map_err(io::Error::other)?;
            loop {
                if entry.th32OwnerProcessID == child.id() {
                    let thread =
                        unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) }
                            .map_err(io::Error::other)?;
                    let thread = unsafe { OwnedHandle::from_raw_handle(thread.0) };
                    if unsafe { ResumeThread(HANDLE(thread.as_raw_handle())) } == u32::MAX {
                        return Err(io::Error::last_os_error());
                    }
                    return Ok(());
                }
                unsafe { Thread32Next(snapshot_handle, &mut entry) }.map_err(io::Error::other)?;
            }
        }

        pub(super) fn terminate(&self) -> io::Result<()> {
            unsafe { TerminateJobObject(HANDLE(self.0.as_raw_handle()), 1) }
                .map_err(io::Error::other)
        }
    }
}
