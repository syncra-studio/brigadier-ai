fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "macos" {
        return;
    }
    // The in-process Apple Event runner and its Objective-C exception
    // boundary for the page tool (src/browser/browser_js.rs). Its own library
    // and symbol names, so it can be linked next to the older helper crates.
    println!("cargo:rerun-if-changed=src/browser/exception_shim.m");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=CoreServices");
    cc::Build::new()
        .file("src/browser/exception_shim.m")
        .flag("-fobjc-arc")
        .compile("cu_layer_browser_exception_shim");

    // Point the linker at the SDK's system sub-library directory so
    // `#[link(name = "dispatch", kind = "dylib")]` can find libdispatch.tbd.
    // SDKROOT takes precedence if set (Xcode builds set it automatically).
    let sdk_root = std::env::var("SDKROOT").unwrap_or_else(|_| {
        std::process::Command::new("xcrun")
            .args(["--sdk", "macosx", "--show-sdk-path"])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .unwrap_or_default()
            .trim()
            .to_owned()
    });
    if !sdk_root.is_empty() {
        println!("cargo:rustc-link-search={sdk_root}/usr/lib/system");
        println!("cargo:rustc-link-search={sdk_root}/usr/lib");
    }
}
