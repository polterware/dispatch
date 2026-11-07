//! whisper.cpp's Metal backend and ONNX Runtime's prebuilt static library
//! call `__isPlatformVersionAtLeast` (from `@available` checks), which lives in
//! clang's compiler runtime. rustc links with `-nodefaultlibs`, so link that
//! runtime explicitly on macOS.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let Ok(out) = std::process::Command::new("clang")
        .arg("--print-resource-dir")
        .output()
    else {
        println!("cargo:warning=clang not found; linking may miss __isPlatformVersionAtLeast");
        return;
    };
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let lib = std::path::Path::new(&dir).join("lib/darwin");
    if lib.join("libclang_rt.osx.a").exists() {
        println!("cargo:rustc-link-search=native={}", lib.display());
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
