use clap::{Subcommand, ValueEnum};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum DemoScenario {
    /// A focused 12-message founder inbox with action, reference, and junk mail.
    FounderInbox,
}

impl DemoScenario {
    #[cfg(feature = "demo")]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::FounderInbox => "founder-inbox",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum DemoAction {
    /// Create a normal workspace wired to the local demo mail server.
    Init {
        /// Workspace path under the current directory.
        #[arg(default_value = "afmail-demo")]
        path: String,
        /// Built-in mailbox scenario.
        #[arg(long, value_enum, default_value = "founder-inbox")]
        scenario: DemoScenario,
        /// Local IMAP port.
        #[arg(long, default_value_t = 14300)]
        imap_port: u16,
        /// Local SMTP port.
        #[arg(long, default_value_t = 14301)]
        smtp_port: u16,
    },
    /// Run the local demo IMAP and SMTP servers until interrupted.
    Serve {
        /// Set from the global --verbose flag after parsing.
        #[arg(skip)]
        verbose: bool,
    },
}
