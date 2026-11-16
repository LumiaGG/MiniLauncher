#![windows_subsystem = "windows"]

// Smoke-test target: records a successful ShellExecute launch without opening UI.
fn main() {
    if let Some(path) = std::env::var_os("MINILAUNCHER_SMOKE_MARKER") {
        std::fs::write(path, "launched").expect("write launch marker");
    }
}
