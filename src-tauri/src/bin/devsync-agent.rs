fn main() {
    if let Err(error) = devsync_lib::run_agent() {
        eprintln!("DevSync Agent failed: {error}");
        std::process::exit(1);
    }
}
