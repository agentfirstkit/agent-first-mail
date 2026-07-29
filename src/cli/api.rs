use clap::{Subcommand, ValueEnum};

/// Network exposure for the local afmail HTTP API.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ApiMode {
    /// Listen only on 127.0.0.1.
    Local,
    /// Listen on the trusted local IPv4 network.
    Lan,
}

#[derive(Subcommand, Debug)]
pub enum ApiAction {
    /// Serve the typed afmail HTTP API and its OpenAPI/JSON Schema documents.
    Serve {
        /// Network exposure mode.
        #[arg(long, value_enum, default_value = "local")]
        mode: ApiMode,
        /// TCP port. Use 0 to let the operating system choose an available port.
        #[arg(long, default_value_t = 9417)]
        port: u16,
        /// Bearer credential. Prefer AFMAIL_API_ACCESS_TOKEN_SECRET to avoid argv exposure.
        #[arg(long)]
        access_token_secret: Option<String>,
    },
    /// Write the generated OpenAPI document and standalone JSON Schemas.
    Export {
        /// Destination directory.
        #[arg(long, default_value = "openapi")]
        directory: String,
        /// Replace existing generated files.
        #[arg(long)]
        force: bool,
    },
}
