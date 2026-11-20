//! Compiles the C bridge and links FFmpeg and libplacebo, both found through pkg-config.

const LIBRARIES: [&str; 7] = [
    "libass",
    "libplacebo",
    "libavformat",
    "libavcodec",
    "libswresample",
    "libavutil",
    "vulkan",
];

fn main() {
    println!("cargo:rerun-if-changed=c/bsv.c");
    println!("cargo:rerun-if-changed=c/bsv.h");

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
    for lib in LIBRARIES {
        let found = pkg_config::Config::new()
            .cargo_metadata(false)
            .probe(lib)
            .unwrap_or_else(|e| panic!("{lib} not found through pkg-config: {e}"));
        for path in &found.include_paths {
            build.include(path);
        }
    }
    build.compile("bsv");

    for lib in LIBRARIES {
        pkg_config::Config::new()
            .probe(lib)
            .expect("probed a moment ago");
    }
}
