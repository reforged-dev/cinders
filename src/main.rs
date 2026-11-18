use std::process::ExitCode;

// musl's allocator takes one lock, which makes a parallel build slower with every thread.
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    // The commands arrive with the stages they run.
    eprintln!("cinders {} has no commands yet", env!("CARGO_PKG_VERSION"));
    ExitCode::FAILURE
}
