use super::*;

impl LiveCli {
    pub fn persist_session(&mut self) -> Result<(), CliError> {
        if self.runtime.session().messages.is_empty() {
            return Ok(());
        }
        if self.runtime.session().title.is_none() {
            let mut guard = self
                .pending_title
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(title) = guard.take() {
                self.runtime.session_mut().title = Some(title);
            }
        }
        self.runtime.session().save_to_path(&self.session.path)?;
        Ok(())
    }

    pub fn switch_to_session_handle(&mut self, handle: SessionHandle) -> Result<usize, CliError> {
        let session = Session::load_from_path(&handle.path)?;
        let message_count = session.messages.len();
        let model = session.model.clone().unwrap_or_else(|| self.model.clone());
        self.runtime = build_runtime(
            session,
            model.clone(),
            self.system_prompt.clone(),
            true,
            self.allowed_tools.clone(),
            self.output_mode.observer(),
        )?;
        self.model = model;
        let _ = runtime::update_settings(|s| {
            s.model = Some(self.model.clone());
        });
        self.session = handle;
        self.title_dispatched = true;
        if let Ok(mut guard) = self.pending_title.lock() {
            *guard = None;
        }
        Ok(message_count)
    }
}

pub(super) fn merge_child_sessions(
    session: &mut Session,
    child_sessions: Vec<runtime::ChildSession>,
) {
    if child_sessions.is_empty() {
        return;
    }
    let parent_model = session.model.clone();
    session
        .child_sessions
        .extend(child_sessions.into_iter().map(|mut child| {
            if child.model.is_none() {
                child.model.clone_from(&parent_model);
            }
            child
        }));
}

#[cfg(test)]
mod tests {
    use super::merge_child_sessions;
    use runtime::{ContentBlock, ConversationMessage, Session};

    #[test]
    fn merge_child_sessions_extends_session() {
        let mut session = Session::new();

        merge_child_sessions(
            &mut session,
            vec![runtime::ChildSession {
                id: "child-1".to_string(),
                model: None,
                goal: "scrape prices".to_string(),
                messages: vec![ConversationMessage::assistant(vec![ContentBlock::Text {
                    text: "done".to_string(),
                }])],
            }],
        );

        assert_eq!(session.child_sessions.len(), 1);
        assert_eq!(session.child_sessions[0].id, "child-1");
    }

    #[test]
    fn merge_child_sessions_inherits_parent_model_when_missing() {
        let mut session = Session::new();
        session.model = Some("anthropic/claude-haiku-4-5".to_string());

        merge_child_sessions(
            &mut session,
            vec![runtime::ChildSession {
                id: "child-1".to_string(),
                model: None,
                goal: "scrape prices".to_string(),
                messages: Vec::new(),
            }],
        );

        assert_eq!(
            session.child_sessions[0].model.as_deref(),
            Some("anthropic/claude-haiku-4-5")
        );
    }
}
