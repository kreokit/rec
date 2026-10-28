// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    rec_lib::install_panic_hook();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = rec_lib::self_test(&args) {
        return code;
    }
    #[cfg(feature = "bench")]
    if let Some(code) = rec_lib::bench_main() {
        std::process::exit(code);
    }
    rec_lib::run();
    std::process::ExitCode::SUCCESS
}
