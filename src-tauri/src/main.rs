// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // A `siphon://` deep-link click spawns a second instance (which the
    // single-instance plugin immediately forwards to the running app and exits).
    // Debug builds are console-subsystem, so that throwaway instance flashes a
    // black console window. Hide it the moment we start so only Siphon shows.
    // Release builds are GUI-subsystem already, so this is a no-op there.
    #[cfg(windows)]
    {
        if std::env::args().any(|a| a.starts_with("siphon://")) {
            hide_console_window();
        }
    }

    siphon_lib::run()
}

#[cfg(windows)]
fn hide_console_window() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> isize;
    }
    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(hwnd: isize, n_cmd_show: i32) -> i32;
    }
    const SW_HIDE: i32 = 0;
    unsafe {
        let hwnd = GetConsoleWindow();
        if hwnd != 0 {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}
