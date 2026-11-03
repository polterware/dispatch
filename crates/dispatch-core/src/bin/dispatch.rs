//! The `dispatch` command on its own, for development and tests; the app
//! bundle runs the same code from its one binary.

fn main() {
    std::process::exit(dispatch_core::cli::main(std::env::args().skip(1).collect()));
}
