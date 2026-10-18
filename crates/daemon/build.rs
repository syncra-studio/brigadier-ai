fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // whisper.cpp's Metal code asks for the macOS version at run time (`@available`) when built
    // for an older macOS than the calls it guards, as an explicit --target build for macOS 14 is.
    // That check (___isPlatformVersionAtLeast) is in clang's runtime library, which rustc
    // doesn't link, so it is linked here.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let runtime = std::process::Command::new("xcrun")
        .args(["clang", "--print-libgcc-file-name"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|path| std::path::Path::new(path).is_file());
    match runtime {
        Some(path) => println!("cargo:rustc-link-arg={path}"),
        None => println!("cargo:warning=clang's runtime library wasn't found; linking may fail"),
    }
}
