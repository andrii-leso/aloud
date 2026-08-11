fn main() {
    // The Windows application manifest. Tauri's built-in one declares only a
    // Common-Controls v6 dependency; `app_manifest` replaces it *wholesale*, so
    // windows-app-manifest.xml re-declares that dependency alongside the DPI,
    // long-path, code-page and execution-level settings (M6 brief section 6.2).
    //
    // The DPI entry is the load-bearing one. tao sets PerMonitorV2 at EventLoop
    // creation, but the mode locks the instant any HWND exists in the process —
    // so an overlay window created earlier would pin the process DPI-unaware,
    // silently, and every capture rectangle on a scaled monitor would be wrong.
    // A manifest is in force before any Rust code runs, which is why it is the
    // primary fix rather than an ordering convention.
    //
    // Deliberately NOT #[cfg(windows)]-gated: a build script is compiled for the
    // HOST, so a cfg here would key off the wrong machine. tauri-build makes the
    // decision itself, from the TARGET triple.
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");

    let attributes = tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("windows-app-manifest.xml")),
    );

    if let Err(error) = tauri_build::try_build(attributes) {
        // Mirrors tauri_build::build()'s own failure path; `{error:#}` prints the
        // full anyhow chain, which is where the useful message lives.
        panic!("failed to run tauri-build: {error:#}");
    }
}
