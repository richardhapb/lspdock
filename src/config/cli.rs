use clap::Parser;

/// LSP Proxy to connect your local environment to Docker
#[derive(Parser, Debug, Default)]
#[command(
    name="lspdock",
    version,
    about = "LSP Proxy to connect your local environment to Docker\n\nUse '--' to separate lspdock args from LSP args when ambiguous.\nExample: lspdock --container app -- --stdio",
    long_about = None,
    author="richardhapb"
)]
pub struct Cli {
    /// Container for attachment
    #[arg(short, long)]
    pub container: Option<String>,
    /// Docker internal path
    #[arg(short, long)]
    pub docker_path: Option<String>,
    /// Local path
    #[arg(short = 'L', long)]
    pub local_path: Option<String>,
    /// Executable for the LSP
    #[arg(short, long)]
    pub exec: Option<String>,
    /// PID patching: indicate the LSPs that require PID patching to null
    #[arg(long)]
    pub pids: Option<Vec<String>>,
    /// Path pattern; this pattern indicates whether Docker will be used
    #[arg(short, long)]
    pub pattern: Option<String>,
    /// Log level: can be trace, debug, info, warning or error
    #[arg(short, long)]
    pub log_level: Option<String>,
    /// Arguments to pass to the LSP
    #[arg(last = true)]
    pub args: Vec<String>,
}

impl Cli {
    /// Parse CLI arguments with fallback to pure LSP passthrough.
    ///
    /// # Parsing Strategy
    ///
    /// 1. Try normal clap parsing
    /// 2. If parsing fails with UnknownArgument:
    ///    - Check if first arg is a known lspdock flag
    ///    - If yes: fail with error (malformed lspdock command)
    ///    - If no: treat all args as LSP passthrough
    ///
    /// # Limitations
    ///
    /// This heuristic can create ambiguity if the LSP uses flags that conflict
    /// with lspdock flags (e.g., `-c`, `-l`, `-p`). In such cases, users MUST
    /// use the `--` separator:
    ///
    /// ```bash
    /// # Ambiguous - is -c for lspdock or LSP?
    /// lspdock -c debug
    ///
    /// # Clear - -c goes to LSP
    /// lspdock -- -c debug
    /// ```
    ///
    /// Users should prefer using `--` for clarity, even when not strictly required.
    pub fn parse() -> Self {
        match Parser::try_parse() {
            Ok(cli) => cli,
            Err(e) => {
                use clap::error::ErrorKind;

                // Only fallback on UnknownArgument errors
                if !matches!(e.kind(), ErrorKind::UnknownArgument) {
                    e.exit();
                }

                let args: Vec<String> = std::env::args().skip(1).collect();

                // Check if first arg is a known lspdock flag
                // If it is, user likely made a mistake with lspdock syntax
                // If not, assume all args are for the LSP
                if let Some(first) = args.first() {
                    let known_flags = [
                        "-c",
                        "--container",
                        "-d",
                        "--docker-path",
                        "-L",
                        "--local-path",
                        "-e",
                        "--exec",
                        "--pids",
                        "-p",
                        "--pattern",
                        "-l",
                        "--log-level",
                        "-h",
                        "--help",
                        "-V",
                        "--version",
                    ];

                    if known_flags.iter().any(|flag| first.starts_with(flag)) {
                        // Started with lspdock flag but parsing failed
                        e.exit();
                    }
                }

                // No lspdock flags detected - treat as pure LSP args
                Self {
                    args,
                    ..Default::default()
                }
            }
        }
    }
}
