// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The Worker mode (ADR 0025): checked BEFORE Tauri init — the
    // Tauri runtime never starts in a Worker.
    if std::env::args().any(|a| a == "--worker") {
        archimedes_lib::run_worker();
        return;
    }
    archimedes_lib::run()
}
