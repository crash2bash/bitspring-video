//! On the Mac, compiles the native interface (`macos/*.swift`) into a static library and links it.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        _ => "x86_64",
    };
    let sdk = output("xcrun", &["--show-sdk-path"]);
    let swiftc = output("xcrun", &["--find", "swiftc"]);

    let mut sources: Vec<PathBuf> = std::fs::read_dir("macos")
        .expect("the macos folder with the interface's sources")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "swift"))
        .collect();
    sources.sort();
    println!("cargo:rerun-if-changed=macos");
    for source in &sources {
        println!("cargo:rerun-if-changed={}", source.display());
    }

    let library = out.join("libbsvinterface.a");
    let optimised = std::env::var("PROFILE").as_deref() == Ok("release");
    let status = Command::new(&swiftc)
        .args([
            "-parse-as-library",
            "-emit-library",
            "-static",
            "-module-name",
            "BSVInterface",
        ])
        .args([
            "-swift-version",
            "5",
            if optimised { "-O" } else { "-Onone" },
        ])
        .args(["-target", &format!("{arch}-apple-macos26.0"), "-sdk", &sdk])
        .arg("-o")
        .arg(&library)
        .args(&sources)
        .status()
        .expect("run swiftc");
    assert!(status.success(), "the native interface did not compile");

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=bsvinterface");
    // The Swift runtime ships with the system; the toolchain adds small compatibility shims.
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    if let Some(toolchain) = PathBuf::from(&swiftc).parent().and_then(|bin| bin.parent()) {
        println!(
            "cargo:rustc-link-search=native={}",
            toolchain.join("lib/swift/macosx").display()
        );
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    // Optimised Swift asks the compiler's runtime which system version it is on (for the
    // "is this API available" checks); Rust's link line does not bring that library along.
    let runtime = PathBuf::from(output(
        "xcrun",
        &["clang", "--print-file-name=libclang_rt.osx.a"],
    ));
    if let Some(folder) = runtime.parent().filter(|_| runtime.exists()) {
        println!("cargo:rustc-link-search=native={}", folder.display());
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
}

fn output(program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run {program}: {e}"));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
