#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Debug)]
pub enum DemoAction {
    /// Create a normal workspace wired to the local demo mail server.
    Init {
        path: String,
        scenario: DemoScenario,
        imap_port: u16,
        smtp_port: u16,
    },
    /// Run the local demo IMAP and SMTP servers until interrupted.
    Serve {
        /// Also report every raw IMAP/SMTP protocol line the demo server sees.
        verbose: bool,
    },
}
