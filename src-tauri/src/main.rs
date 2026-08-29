// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(all(not(debug_assertions), not(feature = "custom-protocol")))]
compile_error!(
    "release builds require Tauri's custom-protocol feature; use `pnpm tauri build --no-bundle` for a smoke-test executable"
);

fn main() {
    main::run()
}
