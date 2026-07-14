use serde_json::Value;

/// A command payload together with the item counts needed to select the
/// terminal protocol event and process exit code.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandResult {
    value: Value,
    succeeded_count: usize,
    failed_count: usize,
}

impl CommandResult {
    pub fn succeeded(value: Value) -> Self {
        Self {
            value,
            succeeded_count: 1,
            failed_count: 0,
        }
    }

    pub fn aggregate(value: Value, succeeded_count: usize, failed_count: usize) -> Self {
        Self {
            value,
            succeeded_count,
            failed_count,
        }
    }

    pub fn is_success(&self) -> bool {
        self.failed_count == 0
    }

    pub fn is_partial(&self) -> bool {
        self.succeeded_count > 0 && self.failed_count > 0
    }

    pub fn exit_code(&self) -> i32 {
        if self.failed_count == 0 {
            0
        } else if self.succeeded_count == 0 {
            1
        } else {
            3
        }
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn succeeded_count(&self) -> usize {
        self.succeeded_count
    }

    pub fn failed_count(&self) -> usize {
        self.failed_count
    }

    pub fn into_value(self) -> Value {
        self.value
    }

    pub fn with_additional_failures(mut self, failed_count: usize) -> Self {
        self.failed_count += failed_count;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn item_counts_select_exit_codes() {
        assert_eq!(CommandResult::aggregate(json!([]), 2, 0).exit_code(), 0);
        assert_eq!(CommandResult::aggregate(json!([]), 1, 1).exit_code(), 3);
        assert_eq!(CommandResult::aggregate(json!([]), 0, 2).exit_code(), 1);
    }
}
