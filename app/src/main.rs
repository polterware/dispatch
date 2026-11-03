fn main() {
    // One binary: as the `dispatch` command, or given a command, it is the
    // CLI and the core (`serve`); otherwise the app.
    let mut argv = std::env::args();
    let argv0 = argv.next().unwrap_or_default();
    let args: Vec<String> = argv.collect();
    if dispatch_core::cli::is_command_line(&argv0, &args) {
        std::process::exit(dispatch_core::cli::main(args));
    }
    dispatch_app::run();
}
