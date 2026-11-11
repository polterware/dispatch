use std::{env, error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    // The repository's schemas: Dispatch's own contracts and DispatchHostService, the one it
    // asks Polterware to serve. Read when the script runs, not baked in when it compiles: a
    // run's worktree shares the target folder, and a script compiled there would keep
    // pointing at that worktree after it is gone.
    let manifest = env::var("CARGO_MANIFEST_DIR")?;
    let schemas = format!("{manifest}/../../schemas");
    let schemas = schemas.as_str();
    let out = PathBuf::from(env::var("OUT_DIR")?);
    let descriptor = out.join("dispatch_descriptor.bin");
    let mut files: Vec<String> = ["dispatch", "health", "workspace"]
        .iter()
        .map(|name| format!("{schemas}/dispatch/v1/{name}.proto"))
        .collect();
    files.push(format!("{schemas}/polterware/local/v1/dispatch_host.proto"));
    println!("cargo:rerun-if-changed={schemas}");
    let mut config = prost_build::Config::new();
    // `PROTOC` names the compiler when it is not on PATH.
    if let Some(protoc) = env::var_os("PROTOC") {
        config.protoc_executable(protoc);
    }
    config.file_descriptor_set_path(&descriptor);
    config.compile_protos(&files, &[schemas])?;
    // A client a release ahead of its core may send a field the core does not know yet:
    // ignoring it keeps older cores answering instead of failing the whole request.
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(descriptor)?)?
        .ignore_unknown_fields()
        .build(&[".dispatch.v1", ".polterware.local.v1"])?;
    Ok(())
}
