// Startup time is the top priority: DLLs that are only needed after the window is up
// are delay-loaded, i.e. mapped on first use (mostly on a background thread) instead of
// at process start.
//
// Security: every DLL that is not one of Windows' "known DLLs" must be delay-loaded.
// Imported directly, Windows would load it before `main` and look for it next to the
// EXE first, a folder any program may write to; delay-loaded, it comes only from
// System32 (`SetDefaultDllDirectories` at the start of `main`). Check with
// `dumpbin /imports target/release/winlaunch.exe`.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    for dll in [
        // COM: app index, icons, unelevated launching
        "ole32.dll",
        "oleaut32.dll",
        "combase.dll",
        // ShellExecute, recycle bin
        "shell32.dll",
        // sleep command
        "powrprof.dll",
        // networking and crypto: only the background update check (Velopack)
        "ws2_32.dll",
        "bcrypt.dll",
        // currency rates, only when a conversion is typed
        "winhttp.dll",
        // not "known DLLs" (see above): window corners/dark mode, Rust's random numbers
        "dwmapi.dll",
        "bcryptprimitives.dll",
    ] {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-lib=delayimp");
}
