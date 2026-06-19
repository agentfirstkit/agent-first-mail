use serde::{Deserialize, Serialize};

pub const CONTACT_KIND: &str = "contact";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContactFrontmatter {
    pub kind: String,
    pub contact_uid: String,
    pub display_name: String,
    #[serde(default)]
    pub emails: Vec<String>,
    #[serde(default)]
    pub phones: Vec<String>,
    #[serde(default)]
    pub organization: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_rfc3339: String,
    pub updated_rfc3339: String,
}

impl ContactFrontmatter {
    pub fn new(contact_uid: &str, display_name: &str, now_rfc3339: &str) -> Self {
        Self {
            kind: CONTACT_KIND.to_string(),
            contact_uid: contact_uid.to_string(),
            display_name: display_name.to_string(),
            emails: Vec::new(),
            phones: Vec::new(),
            organization: String::new(),
            role: String::new(),
            tags: Vec::new(),
            created_rfc3339: now_rfc3339.to_string(),
            updated_rfc3339: now_rfc3339.to_string(),
        }
    }
}
