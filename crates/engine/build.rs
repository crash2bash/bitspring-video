//! Compiles the C bridge and links FFmpeg, libplacebo and libass, all found through pkg-config.

fn main() {
    println!("cargo:rerun-if-changed=c/bsv.c");
    println!("cargo:rerun-if-changed=c/bsv.h");
    println!("cargo:rerun-if-changed=c/between.h");
    println!("cargo:rerun-if-changed=c/between_vt.m");
    println!("cargo:rerun-if-env-changed=BSV_SKIP_NATIVE");

    // For type-checking the Rust side for a system whose libraries are not on this machine
    // (`BSV_SKIP_NATIVE=1 cargo check --target ...`). Nothing built this way can link.
    if std::env::var_os("BSV_SKIP_NATIVE").is_some() {
        return;
    }

    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let apple = std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple");
    let mut libraries = vec![
        "libass",
        "libplacebo",
        "libavformat",
        "libavcodec",
        "libswresample",
        "libavutil",
    ];
    if !windows {
        // Windows renders through Direct3D 11, which is part of the system.
        libraries.push("vulkan");
    }

    let mut build = cc::Build::new();
    build
        .file("c/bsv.c")
        .include("c")
        .std("c11")
        .warnings(true)
        .flag_if_supported("-Wno-unused-parameter");
    // libplacebo's FFmpeg helpers are a header we compile into the bridge; its own code is not ours to lint.
    build.flag_if_supported("-Wno-deprecated-declarations");

    // First pass: include paths only, so the bridge is named to the linker before the
    // libraries it needs (linkers that resolve left to right depend on that order).
    for lib in &libraries {
        let found = pkg_config::Config::new()
            .cargo_metadata(false)
            .probe(lib)
            .unwrap_or_else(|e| panic!("{lib} not found through pkg-config: {e}"));
        for path in &found.include_paths {
            build.include(path);
        }
    }
    build.compile("bsv");

    // The system's maker of frames in between is Objective-C, built apart from the C.
    if apple {
        let mut objc = cc::Build::new();
        objc.file("c/between_vt.m")
            .include("c")
            .flag("-fobjc-arc")
            .warnings(true)
            .flag_if_supported("-Wno-unused-parameter");
        for lib in &libraries {
            let found = pkg_config::Config::new()
                .cargo_metadata(false)
                .probe(lib)
                .expect("probed a moment ago");
            for path in &found.include_paths {
                objc.include(path);
            }
        }
        objc.compile("bsv_between_vt");
        for framework in ["VideoToolbox", "CoreVideo", "CoreMedia", "Foundation"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }

    for lib in &libraries {
        pkg_config::Config::new()
            .probe(lib)
            .expect("probed a moment ago");
    }
}
