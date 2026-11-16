//! Cheap, filesystem-free checks performed before starting a global plugin.
use serde::{Deserialize, Deserializer};

fn lowercase_prefixes<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<String>::deserialize(deserializer)?;
    Ok(values.into_iter().map(|v| v.to_lowercase()).collect())
}

#[derive(Clone, Default, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum GlobalTrigger {
    #[default]
    Always,
    Path,
    Prefix {
        #[serde(deserialize_with = "lowercase_prefixes")]
        values: Vec<String>,
    },
}

impl GlobalTrigger {
    pub fn validate(&self) -> Result<(), String> {
        if let Self::Prefix { values } = self {
            if values.is_empty()
                || values.len() > 32
                || values.iter().any(|value| {
                    value.is_empty() || value.len() > 128 || value.chars().any(char::is_control)
                })
            {
                return Err("global_trigger.prefix 需要 1–32 个非空前缀，每个最多 128 字节且不能包含控制字符".into());
            }
        }
        Ok(())
    }

    pub fn matches(&self, text: &str) -> bool {
        match self {
            Self::Always => true,
            Self::Path => is_path(text),
            Self::Prefix { values } => {
                let text = text.trim_start().to_lowercase();
                values
                    .iter()
                    .any(|prefix| text.starts_with(prefix.as_str()))
            }
        }
    }
}

fn is_path(text: &str) -> bool {
    let text = text.trim().trim_matches(['"', '\'', '“', '‘', '”', '’']);
    let bytes = text.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || text.starts_with(['/', '\\'])
        || text == "~"
        || ["~/", "~\\", "./", ".\\", "../", "..\\"]
            .iter()
            .any(|prefix| text.starts_with(prefix))
        || text
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
        || text
            .strip_prefix('%')
            .and_then(|rest| rest.find('%'))
            .is_some_and(|end| end > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_trigger_accepts_supported_forms_without_matching_regular_text() {
        let trigger = GlobalTrigger::Path;
        for input in [
            r"C:\",
            "d:/Folder",
            r#" "D:\Some Folder\file.txt" "#,
            "‘D:/文件’",
            r"\\server\share",
            "//server/share",
            "~/Desktop",
            "~",
            "\"~\"",
            "%USERPROFILE%/Downloads",
            "FILE:///D:/file.txt",
            "./relative",
            r"..\relative",
        ] {
            assert!(trigger.matches(input), "{input}");
        }
        for input in [
            "",
            "   ",
            "chrome",
            "hello world",
            "https://example.com",
            "1+2",
            "%",
            "%unclosed",
            "12:30",
        ] {
            assert!(!trigger.matches(input), "{input}");
        }
    }

    #[test]
    fn prefix_rules_are_validated_and_case_insensitive() {
        let trigger: GlobalTrigger =
            serde_json::from_str(r#"{"type":"prefix","values":["https://","SSH:"]}"#).unwrap();
        trigger.validate().unwrap();
        assert!(trigger.matches("  HTTPS://example.com"));
        assert!(trigger.matches("ssh:server"));
        assert!(!trigger.matches("chrome"));
        for value in [
            r#"{"type":"prefix","values":[]}"#,
            r#"{"type":"prefix","values":[""]}"#,
        ] {
            assert!(serde_json::from_str::<GlobalTrigger>(value)
                .unwrap()
                .validate()
                .is_err());
        }
        assert!(serde_json::from_str::<GlobalTrigger>(r#"{"type":"unknown"}"#).is_err());
    }
}
