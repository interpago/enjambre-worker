use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::ExitStatusExt;
use std::ptr::null_mut;
use anyhow::Result;
use tracing::{error, info};
use winapi::shared::minwindef::{BOOL, TRUE, LPARAM};
use winapi::um::fileapi::{CreateFileW, OPEN_ALWAYS};
use winapi::um::handleapi::CloseHandle;
use winapi::um::jobapi2::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
};
use winapi::um::processthreadsapi::{
    CreateProcessW, GetExitCodeProcess, STARTUPINFOW, PROCESS_INFORMATION,
};
use winapi::um::winbase::{
    DETACHED_PROCESS, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES,
};
use winapi::um::winnt::{
    HANDLE, JOBOBJECT_BASIC_LIMIT_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    FILE_APPEND_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_ATTRIBUTE_NORMAL,
};
use winapi::um::winuser::{EnumWindows, GetClassNameW, ShowWindow, SW_HIDE};

const STILL_ACTIVE: u32 = 259;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: u32 = 9;

pub struct LlamaProcess {
    process_handle: HANDLE,
    job_handle: HANDLE,
    pid: u32,
}

unsafe impl Send for LlamaProcess {}
unsafe impl Sync for LlamaProcess {}

impl LlamaProcess {
    pub fn spawn(
        program: &std::path::Path,
        args: &[String],
        label: &str,
        stderr_log: Option<&std::path::Path>,
    ) -> Result<Self> {
        let job_name = format!("Global\\WorkerNodeLlamaJob_{}", std::process::id());
        let job_name_wide: Vec<u16> = job_name.encode_utf16().chain(Some(0)).collect();

        let job_handle = unsafe { CreateJobObjectW(null_mut(), job_name_wide.as_ptr()) };
        if job_handle.is_null() {
            let err = std::io::Error::last_os_error();
            error!("CreateJobObjectW falló: {}", err);
            return Err(err.into());
        }

        info!("Job Object creado: {:p}", job_handle);

        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                    | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
                    | JOB_OBJECT_LIMIT_BREAKAWAY_OK,
                ..Default::default()
            },
            ..Default::default()
        };

        let result = unsafe {
            SetInformationJobObject(
                job_handle,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                &mut info as *mut _ as *mut _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };

        if result == 0 {
            let err = std::io::Error::last_os_error();
            error!("SetInformationJobObject falló: {}", err);
            unsafe { CloseHandle(job_handle) };
            return Err(err.into());
        }

        let (h_stdin, h_stdout, h_stderr) = unsafe {
            let nul_name: Vec<u16> = OsStr::new("NUL").encode_wide().chain(Some(0)).collect();
            let h_nul = CreateFileW(
                nul_name.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null_mut(),
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            );

            let h_stderr = match stderr_log {
                Some(path) => {
                    let path_wide: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
                    CreateFileW(
                        path_wide.as_ptr(),
                        FILE_APPEND_DATA,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        null_mut(),
                        OPEN_ALWAYS,
                        FILE_ATTRIBUTE_NORMAL,
                        null_mut(),
                    )
                }
                None => h_nul,
            };

            (h_nul, h_nul, h_stderr)
        };

        let mut cmd_line = format!("\"{}\"", program.to_string_lossy());
        for arg in args {
            if arg.contains(' ') {
                cmd_line.push_str(&format!(" \"{}\"", arg));
            } else {
                cmd_line.push(' ');
                cmd_line.push_str(arg);
            }
        }
        let mut cmd_wide: Vec<u16> = OsStr::new(&cmd_line).encode_wide().chain(Some(0)).collect();
        let prog_wide: Vec<u16> = OsStr::new(program.as_os_str()).encode_wide().chain(Some(0)).collect();

        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESHOWWINDOW | STARTF_USESTDHANDLES;
        si.wShowWindow = SW_HIDE as u16;
        si.hStdInput = h_stdin;
        si.hStdOutput = h_stdout;
        si.hStdError = h_stderr;

        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

        let success = unsafe {
            CreateProcessW(
                prog_wide.as_ptr(),
                cmd_wide.as_mut_ptr(),
                null_mut(),
                null_mut(),
                TRUE,
                DETACHED_PROCESS,
                null_mut(),
                null_mut(),
                &mut si,
                &mut pi,
            )
        };

        if success == 0 {
            let err = std::io::Error::last_os_error();
            error!("CreateProcessW falló para {}: {}", label, err);
            unsafe {
                CloseHandle(h_stdin);
                CloseHandle(h_stdout);
                if h_stderr != h_stdin {
                    CloseHandle(h_stderr);
                }
                CloseHandle(job_handle);
            }
            return Err(err.into());
        }

        let pid = pi.dwProcessId;
        info!("{label} lanzado (PID: {pid})");

        unsafe { CloseHandle(pi.hThread) };

        let assigned = unsafe { AssignProcessToJobObject(job_handle, pi.hProcess) };
        if assigned == 0 {
            let err = std::io::Error::last_os_error();
            error!("AssignProcessToJobObject falló: {}", err);
            unsafe {
                CloseHandle(pi.hProcess);
                CloseHandle(h_stdin);
                CloseHandle(h_stdout);
                if h_stderr != h_stdin {
                    CloseHandle(h_stderr);
                }
                CloseHandle(job_handle);
            }
            return Err(err.into());
        }

        info!("PID {} asignado al Job Object", pid);

        unsafe {
            CloseHandle(h_stdin);
            CloseHandle(h_stdout);
            if h_stderr != h_stdin {
                CloseHandle(h_stderr);
            }
        }

        // Cazar cualquier ventana de consola que aparezca del proceso hijo
        suppress_child_console(pid);

        Ok(Self {
            process_handle: pi.hProcess,
            job_handle,
            pid,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        let mut exit_code: u32 = 0;
        let success = unsafe { GetExitCodeProcess(self.process_handle, &mut exit_code) };
        if success == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if exit_code == STILL_ACTIVE {
            return Ok(None);
        }
        Ok(Some(std::process::ExitStatus::from_raw(exit_code)))
    }
}

impl Drop for LlamaProcess {
    fn drop(&mut self) {
        info!("Drop del Job Object para PID {} — el hijo será terminado", self.pid);

        if !self.job_handle.is_null() {
            unsafe { CloseHandle(self.job_handle) };
            self.job_handle = null_mut();
        }

        let mut exit_code: u32 = 0;
        unsafe { GetExitCodeProcess(self.process_handle, &mut exit_code) };
        if exit_code == STILL_ACTIVE {

        }

        if !self.process_handle.is_null() {
            unsafe { CloseHandle(self.process_handle) };
            self.process_handle = null_mut();
        }
    }
}

fn suppress_child_console(child_pid: u32) {
    // Como somos GUI subsystem, no tenemos consola propia.
    // Cazamos cualquier ventana ConsoleWindowClass que aparezca y la escondemos.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));

        unsafe {
            for _ in 0..8 {
                EnumWindows(Some(enum_hide_console), child_pid as LPARAM);
                std::thread::sleep(std::time::Duration::from_millis(75));
            }
        }
    });
}

unsafe extern "system" fn enum_hide_console(hwnd: winapi::shared::windef::HWND, _lparam: LPARAM) -> BOOL {
    let mut class_buf: [u16; 256] = [0; 256];
    let len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256);
    if len == 0 {
        return TRUE;
    }
    let class_name = String::from_utf16_lossy(&class_buf[..len as usize]);

    // "ConsoleWindowClass" = consola clasica (conhost.exe)
    // "Windows.UI.Core.CoreWindow" = terminal moderna
    if class_name != "ConsoleWindowClass" && class_name != "Windows.UI.Core.CoreWindow" {
        return TRUE;
    }

    ShowWindow(hwnd, SW_HIDE);
    TRUE
}
