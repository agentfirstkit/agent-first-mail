/// Network exposure for the local afmail HTTP API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiMode {
    /// Listen only on 127.0.0.1.
    Local,
    /// Listen on the trusted local IPv4 network.
    Lan,
}

#[derive(Debug)]
pub enum ApiAction {
    /// Serve the typed afmail HTTP API and its OpenAPI/JSON Schema documents.
    Serve {
        mode: ApiMode,
        port: u16,
        access_token_secret: Option<String>,
    },
    /// Write the generated OpenAPI document and standalone JSON Schemas.
    Export { directory: String, force: bool },
}
