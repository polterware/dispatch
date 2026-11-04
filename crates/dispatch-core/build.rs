use std::{env, error::Error, path::PathBuf};

// The repository's schemas: Dispatch's own contracts and DispatchHostService, the one it
// asks Polterware to serve.
const SCHEMAS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../schemas");

fn main() -> Result<(), Box<dyn Error>> {
    let out = PathBuf::from(env::var("OUT_DIR")?);
    let descriptor = out.join("dispatch_descriptor.bin");
    let mut files: Vec<String> = ["dispatch", "health", "workspace"]
        .iter()
        .map(|name| format!("{SCHEMAS}/dispatch/v1/{name}.proto"))
        .collect();
    files.push(format!("{SCHEMAS}/polterware/local/v1/dispatch_host.proto"));
    println!("cargo:rerun-if-changed={SCHEMAS}");
    let mut config = prost_build::Config::new();
    // `PROTOC` names the compiler when it is not on PATH.
    if let Some(protoc) = env::var_os("PROTOC") {
        config.protoc_executable(protoc);
    }
    config.file_descriptor_set_path(&descriptor);
    config.compile_protos(&files, &[SCHEMAS])?;
    // A client a release ahead of its core may send a field the core does not know yet:
    // ignoring it keeps older cores answering instead of failing the whole request.
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(descriptor)?)?
        .ignore_unknown_fields()
        .build(&[".dispatch.v1", ".polterware.local.v1"])?;
    Ok(())
}
