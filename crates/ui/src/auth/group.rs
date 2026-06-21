use super::builder::CredGroup;

#[must_use]
pub fn flag_group_for(provider_id: &str) -> Option<CredGroup> {
    match provider_id {
        "amazon-bedrock" => Some(CredGroup::Bedrock),
        "azure" => Some(CredGroup::Azure),
        "vertex" => Some(CredGroup::Vertex),
        "other" | "custom" => Some(CredGroup::Custom),
        // `claude`/`gpt` are legacy aliases with no matching preset id.
        "anthropic" | "claude" | "openai" | "gpt" => Some(CredGroup::Simple),
        other => api::find_preset(other).map(|_| CredGroup::Simple),
    }
}

#[must_use]
pub fn is_scriptable(provider_id: &str) -> bool {
    provider_id != "copilot"
}

#[cfg(test)]
mod tests {
    use super::{flag_group_for, is_scriptable, CredGroup};

    #[test]
    fn every_builtin_preset_maps_to_a_group() {
        for preset in api::builtin_presets() {
            assert!(
                flag_group_for(preset.id).is_some(),
                "preset `{}` should map to a credential group",
                preset.id
            );
        }
    }

    #[test]
    fn enterprise_providers_map_to_dedicated_groups() {
        assert_eq!(flag_group_for("amazon-bedrock"), Some(CredGroup::Bedrock));
        assert_eq!(flag_group_for("azure"), Some(CredGroup::Azure));
        assert_eq!(flag_group_for("vertex"), Some(CredGroup::Vertex));
    }

    #[test]
    fn custom_aliases_map_to_custom() {
        assert_eq!(flag_group_for("other"), Some(CredGroup::Custom));
        assert_eq!(flag_group_for("custom"), Some(CredGroup::Custom));
    }

    #[test]
    fn legacy_aliases_map_to_simple() {
        assert_eq!(flag_group_for("anthropic"), Some(CredGroup::Simple));
        assert_eq!(flag_group_for("claude"), Some(CredGroup::Simple));
        assert_eq!(flag_group_for("openai"), Some(CredGroup::Simple));
        assert_eq!(flag_group_for("gpt"), Some(CredGroup::Simple));
    }

    #[test]
    fn registered_preset_maps_to_simple() {
        assert_eq!(flag_group_for("google"), Some(CredGroup::Simple));
        assert_eq!(flag_group_for("groq"), Some(CredGroup::Simple));
        assert_eq!(flag_group_for("copilot"), Some(CredGroup::Simple));
    }

    #[test]
    fn unknown_provider_maps_to_none() {
        assert_eq!(flag_group_for("does-not-exist"), None);
        assert_eq!(flag_group_for(""), None);
    }

    #[test]
    fn only_copilot_is_non_scriptable() {
        assert!(!is_scriptable("copilot"));
        assert!(is_scriptable("anthropic"));
        assert!(is_scriptable("openai"));
        assert!(is_scriptable("amazon-bedrock"));
        assert!(is_scriptable("other"));
        assert!(is_scriptable("does-not-exist"));
    }
}
