fn main() {
    #[cfg(feature = "napi")]
    {
        napi_build::setup();
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
            // The Tauri crate compiles this same source for itself; it never turns this feature on.
            let source = "../../apps/desktop/src-tauri/src/notifications.m";
            cc::Build::new()
                .file(source)
                .flag("-fobjc-arc")
                .flag("-fblocks")
                .compile("brigadier_notifications");
            println!("cargo:rustc-link-lib=framework=UserNotifications");
            println!("cargo:rustc-link-lib=framework=Foundation");
            println!("cargo:rerun-if-changed={source}");
        }
    }
}
