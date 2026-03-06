fn main() {
    #[cfg(not(feature = "native"))]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--help" || a == "-h") {
            println!("mxdx-launcher - Matrix-native fleet management launcher");
            println!("Usage: mxdx-launcher [OPTIONS]");
            println!("  --help     Print this help");
            println!("  --version  Print version");
            println!();
            println!("Note: WASI build has limited functionality.");
            println!("Full features require native build.");
            return;
        }
        if args.iter().any(|a| a == "--version" || a == "-V") {
            println!("mxdx-launcher {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        eprintln!("mxdx-launcher WASI build: Use --help for available options.");
        eprintln!("Matrix networking not available in this distribution.");
        eprintln!("Use the native binary for production use.");
        std::process::exit(1);
    }

    #[cfg(feature = "native")]
    {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            println!("mxdx-launcher starting...");
        });
    }
}
