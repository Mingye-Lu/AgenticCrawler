use crate::ToolEffect;

/// Observable result returned by a tool execution.
#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub text: String,
    pub effect: Option<ToolEffect>,
}

impl ToolOutcome {
    #[must_use]
    pub fn reply(text: String) -> Self {
        Self { text, effect: None }
    }

    #[must_use]
    pub fn with_effect(text: String, effect: ToolEffect) -> Self {
        Self {
            text,
            effect: Some(effect),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolOutcome;
    use crate::ToolEffect;

    #[test]
    fn reply_creates_no_effect_outcome() {
        let outcome = ToolOutcome::reply("ok".to_string());

        assert_eq!(outcome.text, "ok");
        assert!(outcome.effect.is_none());
    }

    #[test]
    fn with_effect_sets_effect() {
        let outcome = ToolOutcome::with_effect(
            "waiting".to_string(),
            ToolEffect::Pause {
                reason: "need confirmation".to_string(),
            },
        );

        match outcome.effect {
            Some(ToolEffect::Pause { reason }) => assert_eq!(reason, "need confirmation"),
            _ => panic!("expected Pause effect"),
        }
    }

    #[test]
    fn text_field_is_publicly_accessible() {
        let outcome = ToolOutcome::reply("visible text".to_string());

        assert_eq!(outcome.text, "visible text");
    }
}
