pub const SECRET_FIELDS: &[&str] = &["api_key", "aws_secret_access_key"];

#[must_use]
pub fn mask_secret(s: &str) -> String {
    let count = s.chars().count();
    if count <= 4 {
        return "••••".to_string();
    }
    let tail: String = s.chars().skip(count - 4).collect();
    format!("••••{tail}")
}

#[cfg(test)]
mod tests {
    use super::{mask_secret, SECRET_FIELDS};

    #[test]
    fn masks_long_secret_to_last_four() {
        assert_eq!(mask_secret("sk-ant-abc1234"), "••••1234");
    }

    #[test]
    fn empty_and_short_secrets_fully_masked() {
        assert_eq!(mask_secret(""), "••••");
        assert_eq!(mask_secret("a"), "••••");
        assert_eq!(mask_secret("abcd"), "••••");
    }

    #[test]
    fn five_char_secret_shows_last_four() {
        assert_eq!(mask_secret("abcde"), "••••bcde");
    }

    #[test]
    fn never_reveals_more_than_four_chars() {
        let masked = mask_secret("super-secret-value-9876");
        assert!(masked.ends_with("9876"));
        assert!(!masked.contains("value"));
    }

    #[test]
    fn secret_fields_lists_known_secrets() {
        assert!(SECRET_FIELDS.contains(&"api_key"));
        assert!(SECRET_FIELDS.contains(&"aws_secret_access_key"));
    }
}
