use crate::cli::DemoScenario;
use crate::error::{AppError, Result};

pub(super) const DEMO_IDENTITY_NAME: &str = "Alex Founder";
pub(super) const DEMO_IDENTITY_EMAIL: &str = "founder@example.com";
pub(super) const DEMO_USERNAME: &str = "founder";

const FOUNDER_INBOX_MESSAGES: [&[u8]; 12] = [
    include_bytes!("../../tests/fixtures/mail-batch/eml/001_refund_initial.eml"),
    include_bytes!("../../tests/fixtures/mail-batch/eml/002_refund_followup.eml"),
    include_bytes!("../../tests/fixtures/mail-batch/eml/003_contract_attachment.eml"),
    include_bytes!("../../tests/fixtures/mail-batch/eml/005_receipt_cloud.eml"),
    include_bytes!("../../tests/fixtures/mail-batch/eml/007_newsletter_product.eml"),
    include_bytes!("../../tests/fixtures/mail-batch/eml/008_login_code.eml"),
    include_bytes!("../../tests/fixtures/mail-batch/eml/011_vip_escalation.eml"),
    include_bytes!("../../tests/fixtures/mail-batch/eml/013_invoice_overdue.eml"),
    include_bytes!("../../tests/fixtures/mail-batch/eml/015_outage_notice.eml"),
    include_bytes!("../../tests/fixtures/mail-batch/eml/017_inbox_password_phish.eml"),
    include_bytes!("../../tests/fixtures/mail-batch/eml/020_inbox_fake_invoice.eml"),
    include_bytes!("../../tests/fixtures/demo/bug_report.eml"),
];

#[derive(Clone)]
pub(super) struct SeedMessage {
    pub(super) mailbox_name: &'static str,
    pub(super) raw_eml: Vec<u8>,
}

pub(super) fn message_count(scenario: DemoScenario) -> usize {
    match scenario {
        DemoScenario::FounderInbox => FOUNDER_INBOX_MESSAGES.len(),
    }
}

pub(super) fn seed_messages(scenario: DemoScenario) -> Result<Vec<SeedMessage>> {
    match scenario {
        DemoScenario::FounderInbox => FOUNDER_INBOX_MESSAGES
            .iter()
            .map(|raw| {
                let text = std::str::from_utf8(raw).map_err(|error| {
                    AppError::new(
                        "demo_fixture_invalid",
                        format!("demo fixture is not UTF-8: {error}"),
                    )
                })?;
                let raw_eml = text
                    .replace("me@localhost", DEMO_IDENTITY_EMAIL)
                    .replace(
                        "Me <founder@example.com>",
                        "Alex Founder <founder@example.com>",
                    )
                    .into_bytes();
                if mail_parser::MessageParser::default()
                    .parse(&raw_eml)
                    .is_none()
                {
                    return Err(AppError::new(
                        "demo_fixture_invalid",
                        "demo fixture could not be parsed as RFC 5322 mail",
                    ));
                }
                Ok(SeedMessage {
                    mailbox_name: "INBOX",
                    raw_eml,
                })
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{DEMO_IDENTITY_EMAIL, message_count, seed_messages};
    use crate::cli::DemoScenario;

    #[test]
    fn founder_inbox_is_a_focused_twelve_message_scenario() {
        let scenario = DemoScenario::FounderInbox;
        assert_eq!(message_count(scenario), 12);
        let messages = seed_messages(scenario).expect("demo fixtures");
        assert_eq!(messages.len(), 12);
        assert!(
            messages
                .iter()
                .all(|message| !String::from_utf8_lossy(&message.raw_eml).contains("me@localhost"))
        );
        assert!(messages.iter().all(|message| {
            String::from_utf8_lossy(&message.raw_eml).contains(DEMO_IDENTITY_EMAIL)
        }));
    }
}
