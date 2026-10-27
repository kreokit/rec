use std::env;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_macos_recorder();
    }
    tauri_build::build()
}

/// The macOS recorder (`native/macos`, Objective-C) behind the C interface in recorder.h.
/// ScreenCaptureKit is linked weakly: it exists from macOS 12.3, the app runs from 12.0
/// and uses it only on 13+ (scripts/check_macos_binary.py allows exactly this framework).
fn build_macos_recorder() {
    println!("cargo:rerun-if-changed=native/macos");
    let mut build = cc::Build::new();
    build.file("native/macos/recorder.m");
    build.file("native/macos/window.m");
    println!("cargo:rustc-link-lib=framework=AppKit");
    if env::var_os("CARGO_FEATURE_BENCH").is_some() {
        build.file("native/macos/bench.m");
    }
    build
        .flag("-fobjc-arc")
        .flag("-mmacosx-version-min=12.0")
        .flag("-Wno-unused-parameter")
        .opt_level(2)
        .compile("kreokit_recorder");
    for framework in [
        "AVFoundation",
        "CoreGraphics",
        "CoreMedia",
        "CoreVideo",
        "CoreAudio",
        "CoreImage",
        "Foundation",
        "ImageIO",
        "IOSurface",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!("cargo:rustc-link-arg=-Wl,-weak_framework,ScreenCaptureKit");
}
