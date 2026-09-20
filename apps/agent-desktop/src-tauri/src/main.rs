fn main() {
    if let Err(error) = agent_desktop::run() {
        eprintln!("desktop startup failed: {error}");
        std::process::exit(1);
    }
}
