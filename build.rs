//! 只在 macOS 上把 swift/*.swift 编成静态库链进 dct：安全芯片只能从 CryptoKit 用。
//! 别的平台什么都不做——Windows、Linux 的构建不需要 Swift，也不该需要。
use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        a => panic!("unsupported arch {a}"),
    };
    let mut sources: Vec<PathBuf> = std::fs::read_dir("swift")
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "swift"))
        .collect();
    sources.sort();
    println!("cargo:rerun-if-changed=swift");
    for s in &sources {
        println!("cargo:rerun-if-changed={}", s.display());
    }
    let lib = out.join("libDctMac.a");
    let status = Command::new("xcrun")
        .args(["swiftc", "-parse-as-library", "-emit-library", "-static", "-module-name", "DctMac", "-swift-version", "5", "-O"])
        // macOS 11.0 目标下，swiftc 默认会自动链接 Swift 5.6/并发/动态替换这几个向后
        // 兼容库（swiftCompatibility56、swiftCompatibilityConcurrency 等）；Command
        // Line Tools 不带这几个静态库，链接会报符号找不到。Keys.swift 没用并发、也没用
        // 动态替换，关掉这三项自动链接就行，不需要装完整 Xcode。
        .args([
            "-disable-autolinking-runtime-compatibility",
            "-disable-autolinking-runtime-compatibility-concurrency",
            "-disable-autolinking-runtime-compatibility-dynamic-replacements",
        ])
        .args(["-target", &format!("{arch}-apple-macos11.0"), "-o"])
        .arg(&lib)
        .args(&sources)
        .status()
        .expect("run xcrun swiftc");
    assert!(status.success(), "swiftc failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=DctMac");
    let sdk = Command::new("xcrun").args(["--show-sdk-path"]).output().expect("xcrun --show-sdk-path").stdout;
    println!("cargo:rustc-link-search=native={}/usr/lib/swift", String::from_utf8(sdk).unwrap().trim());
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    for f in ["CryptoKit", "Foundation", "LocalAuthentication", "Security"] {
        println!("cargo:rustc-link-lib=framework={f}");
    }
}
