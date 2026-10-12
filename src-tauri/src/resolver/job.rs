//! Keep yt-dlp and any Deno descendants inside one kill-on-close Windows job.
//! Cancelling a resolution must not leave helper processes behind after Quit.
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows::Win32::{
    Foundation::HANDLE,
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
};

pub(super) struct ResolverJob(OwnedHandle);
impl ResolverJob {
    pub fn attach(child: &mut tokio::process::Child) -> Result<Option<Self>, String> {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Ok(None);
        }
        let process = child.raw_handle().ok_or("Resolver already exited.")?;
        // SAFETY: the child process handle is live and borrowed. The job handle
        // is transferred exactly once into OwnedHandle, including on errors.
        unsafe {
            let handle =
                CreateJobObjectW(None, None).map_err(|_| "Cannot create resolver process job.")?;
            let job = Self(OwnedHandle::from_raw_handle(handle.0));
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                HANDLE(job.0.as_raw_handle()),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            )
            .map_err(|_| "Cannot configure resolver process job.")?;
            if AssignProcessToJobObject(HANDLE(job.0.as_raw_handle()), HANDLE(process)).is_err() {
                // Very short version probes may have exited between spawn and
                // assignment; they no longer need containment.
                if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                    return Ok(None);
                }
                return Err("Cannot contain resolver process tree.".into());
            }
            Ok(Some(job))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropping_job_terminates_pending_resolver() {
        tauri::async_runtime::block_on(async {
            let mut child = tokio::process::Command::new("cmd.exe")
                .args(["/c", "ping", "-n", "60", "127.0.0.1"])
                .creation_flags(0x08000000)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let job = ResolverJob::attach(&mut child).unwrap().unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(child.try_wait().unwrap().is_none());
            drop(job);
            tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
                .await
                .unwrap()
                .unwrap();
        });
    }
}
