fn main() {
    #[cfg(target_os = "linux")]
    if let Err(error) = oojuro::ojo_journal::cli::run() {
        eprintln!("ojo-journal: {error}");
        std::process::exit(2);
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("ojo-journal: unsupported_platform");
        std::process::exit(2);
    }
}
