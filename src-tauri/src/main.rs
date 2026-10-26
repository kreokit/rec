// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(feature = "bench")]
    if let Some(code) = rec_lib::bench_main() {
        std::process::exit(code);
    }
    rec_lib::run();
}
