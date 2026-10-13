fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/overnight_notifications.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("brigadier_run_notifications");
        println!("cargo:rustc-link-lib=framework=UserNotifications");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rerun-if-changed=src/overnight_notifications.m");
        cc::Build::new()
            .file("src/login_item.m")
            .flag("-fobjc-arc")
            .compile("brigadier_login_item");
        println!("cargo:rustc-link-lib=framework=ServiceManagement");
        println!("cargo:rerun-if-changed=src/login_item.m");
        cc::Build::new()
            .file("src/computer_setup.m")
            .flag("-fobjc-arc")
            .compile("brigadier_computer_setup");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rerun-if-changed=src/computer_setup.m");
    }
    tauri_build::build();
}
