// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Broker mode: the same exe, started elevated through UAC, serving raw-disk requests over
    // the UI's pipe. It opens no window and exits when the UI goes away.
    if args.iter().any(|a| a == unlost_core::broker::FLAG) {
        std::process::exit(unlost_core::broker::run(&args));
    }
    unlost_app::run();
}
