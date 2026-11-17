//! Control commands: stop the running instance, set up / remove autostart.
//!
//! Autostart is a Task Scheduler task ("At log on", highest privileges) instead of
//! the Run key / Startup folder: it starts without Explorer's startup delay, and
//! running elevated lets the keyboard hook see Win+Space over admin windows too.

use crate::i18n::{t, t_args};
use crate::{wide, CLASS_NAME, QUIT_MSG_NAME, TASK_NAME, UNINSTALL_MSG_NAME};
use std::mem::{size_of, zeroed};
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Asks a running instance to exit and waits (up to 5 s) until it is gone.
/// Returns true if an instance was running.
pub fn quit_running() -> bool {
    stop_running(QUIT_MSG_NAME)
}

/// Velopack uninstall hook: the running instance deletes the autostart task and exits.
/// The task was created with admin rights; the hook itself runs without them and may
/// not ask (no UI in hooks), but the autostart instance is elevated. If no instance
/// runs, or it is not elevated, the hook tries itself as a fallback.
pub fn uninstall_running() {
    stop_running(UNINSTALL_MSG_NAME);
    remove_autostart_quietly();
}

/// Posts the registered message `msg_name` to a running instance (which then exits)
/// and waits up to 5 s for it to end. Returns true if an instance was running.
fn stop_running(msg_name: &str) -> bool {
    unsafe {
        let hwnd = FindWindowW(wide(CLASS_NAME).as_ptr(), null());
        if hwnd.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        // SYNCHRONIZE is granted even on an elevated process.
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        let msg = RegisterWindowMessageW(wide(msg_name).as_ptr());
        PostMessageW(hwnd, msg, 0, 0);

        if !process.is_null() {
            WaitForSingleObject(process, 5000);
            CloseHandle(process);
        } else {
            let start = Instant::now();
            while IsWindow(hwnd) != 0 && start.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        true
    }
}

/// `--install`: elevates if needed, then registers and starts the autostart task.
pub fn install_cmd() -> i32 {
    let user = current_user();
    if is_elevated() {
        install(&user)
    } else {
        run_elevated(&format!("--install-elevated \"{user}\""))
    }
}

/// `--uninstall`: elevates if needed, then stops the launcher and removes the task.
pub fn uninstall_cmd() -> i32 {
    if is_elevated() {
        uninstall()
    } else {
        run_elevated("--uninstall-elevated")
    }
}

/// Elevated part of `--install`. `user` comes from the non-elevated caller so the
/// task is registered for the logged-in user, not for an "over the shoulder" admin.
pub fn install(user: &str) -> i32 {
    let result = (|| -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        quit_running();
        register_and_run_task(task_xml(user, &exe.to_string_lossy()))
    })();

    match result {
        Ok(()) => {
            message(&t("install-success"), false);
            0
        }
        Err(e) => {
            message(&t_args("install-failed", &[("details", &e)]), true);
            1
        }
    }
}

/// Elevated part of `--uninstall`.
pub fn uninstall() -> i32 {
    quit_running();
    match schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]) {
        Ok(()) => {
            message(&t("uninstall-success"), false);
            0
        }
        Err(e) => {
            message(&t_args("uninstall-failed", &[("details", &e)]), true);
            1
        }
    }
}

/// Removes the autostart task without any UI (no UAC prompt). Without admin rights
/// Windows may refuse; the task then stays behind and finds no EXE at the next sign-in.
pub fn remove_autostart_quietly() {
    let _ = schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]);
}

/// Registers (or replaces) the autostart task and starts it, through the Task Scheduler
/// COM API. The definition is handed over in memory: an XML file in %TEMP% (as with
/// `schtasks /XML`) could be swapped by any unelevated program between writing and
/// reading, and the elevated task would then run its command at every sign-in.
fn register_and_run_task(xml: String) -> Result<(), String> {
    // Own thread: COM must be initialized there, whatever the caller's thread uses.
    std::thread::spawn(move || unsafe {
        use windows::core::BSTR;
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_MULTITHREADED,
        };
        use windows::Win32::System::TaskScheduler::*;
        use windows::Win32::System::Variant::VARIANT;

        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> windows::core::Result<()> {
            let service: ITaskService =
                CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            let none = VARIANT::default();
            service.Connect(&none, &none, &none, &none)?;
            let folder = service.GetFolder(&BSTR::from("\\"))?;
            let task = folder.RegisterTask(
                &BSTR::from(TASK_NAME),
                &BSTR::from(xml.as_str()),
                TASK_CREATE_OR_UPDATE.0,
                &none, // user and logon type come from the XML's principal
                &none,
                TASK_LOGON_INTERACTIVE_TOKEN,
                &none,
            )?;
            task.Run(&none)?;
            Ok(())
        })();
        if initialized {
            CoUninitialize();
        }
        result.map_err(|e| e.message())
    })
    .join()
    .unwrap_or_else(|_| Err("task registration crashed".into()))
}

fn task_xml(user: &str, exe: &str) -> String {
    let user = xml_escape(user);
    let exe = xml_escape(exe);
    let description = xml_escape(&t("task-description"));
    // Priority 4 = normal process priority (the task default, 7, is below normal).
    // ExecutionTimeLimit PT0S = never stop (the default kills the task after 72 h).
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{description}</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn schtasks(args: &[&str]) -> Result<(), String> {
    let out = Command::new(crate::system_exe("schtasks.exe"))
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| t_args("error-schtasks-start", &[("details", &e.to_string())]))?;
    if out.status.success() {
        Ok(())
    } else {
        let text = String::from_utf8_lossy(&out.stderr);
        let text = if text.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout)
        } else {
            text
        };
        Err(text.trim().to_string())
    }
}

fn current_user() -> String {
    let name = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
        _ => name,
    }
}

pub(crate) fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation: TOKEN_ELEVATION = zeroed();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as _,
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// Restarts this EXE with admin rights (UAC prompt) and returns its exit code.
/// Returns 1 if the user cancels the UAC prompt.
///
/// The "runas" verb only works on a single-threaded COM apartment (STA). On other
/// threads, e.g. one where a library already set up COM differently, ShellExecuteEx
/// fails at once with ERROR_CANCELLED without ever showing the UAC prompt. So this
/// always runs on its own freshly initialized STA thread.
fn run_elevated(params: &str) -> i32 {
    let params = params.to_string();
    std::thread::spawn(move || unsafe {
        use windows::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        };
        let initialized =
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
        let code = run_elevated_sta(&params);
        if initialized {
            CoUninitialize();
        }
        code
    })
    .join()
    .unwrap_or(1)
}

fn run_elevated_sta(params: &str) -> i32 {
    unsafe {
        let Ok(exe) = std::env::current_exe() else {
            return 1;
        };
        let exe = wide(&exe.to_string_lossy());
        let verb = wide("runas");
        let params = wide(params);
        let mut sei: SHELLEXECUTEINFOW = zeroed();
        sei.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        sei.fMask = SEE_MASK_NOCLOSEPROCESS;
        sei.lpVerb = verb.as_ptr();
        sei.lpFile = exe.as_ptr();
        sei.lpParameters = params.as_ptr();
        sei.nShow = SW_NORMAL;
        if ShellExecuteExW(&mut sei) == 0 || sei.hProcess.is_null() {
            crate::debug_log::log(&format!(
                "ShellExecuteExW(runas) failed: error {}",
                GetLastError()
            ));
            return 1; // UAC cancelled
        }
        WaitForSingleObject(sei.hProcess, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(sei.hProcess, &mut code);
        CloseHandle(sei.hProcess);
        code as i32
    }
}

fn message(text: &str, error: bool) {
    let icon = if error {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(text).as_ptr(),
            wide("WinLaunch").as_ptr(),
            MB_OK | icon,
        );
    }
}
