//! Stdio entry point for `aifs-engine`.

fn main() {
    if let Err(error) = aifs_engine::run_stdio() {
        eprintln!("aifs-engine: {error}");
        std::process::exit(1);
    }
}
