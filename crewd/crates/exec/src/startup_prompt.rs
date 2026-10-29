//! Diagnostic observations only: never a readiness or input-authority decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum StartupPrompt {
    Unclassified = 0,
    FolderTrust = 1,
    OriginalResumeRejected = 2,
    Unavailable = 3,
}

pub(crate) fn classify(text: &str, original: &str) -> StartupPrompt {
    if text.len() > 16 * 1024 {
        return StartupPrompt::Unavailable;
    }
    if crate::resume::valid_agent_session_id(original)
        && text.lines().map(str::trim).any(|line| {
            line.strip_prefix(crate::session::NO_CONVERSATION_MARKER)
                .is_some_and(|tail| {
                    tail == format!(": {original}") || tail == format!(" {original}")
                })
        })
    {
        return StartupPrompt::OriginalResumeRejected;
    }
    if let Some(menu) = crewd_term::parse_menu(text) {
        let prompt = menu.prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        let numbered = prompt == "Do you trust the files in this folder?"
            && menu.options == ["Yes, proceed", "No, exit"];
        let unnumbered = matches!(
            prompt.as_str(),
            "Quick safety check: Is this a project you created or one you trust?"
                | "Quick safety check: Is this a project you created or one you trust? (Like your"
        ) && menu.options == ["No, exit", "Yes, I trust this folder"];
        if numbered || unnumbered {
            return StartupPrompt::FolderTrust;
        }
    }
    StartupPrompt::Unclassified
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_prompt_requires_complete_known_menu_or_original_resume_id() {
        let id = "6e290649-86d4-4136-94b4-305c3a892330";
        assert_eq!(
            classify(
                "Do you trust the files in this folder?\n❯ 1. Yes, proceed\n  2. No, exit",
                id
            ),
            StartupPrompt::FolderTrust
        );
        assert_eq!(
            classify(
                "Quick safety check: Is this a project you created or one you trust?\n\n❯ No, exit\n  Yes, I trust this folder\n\nEnter to confirm · Esc to cancel",
                id
            ),
            StartupPrompt::FolderTrust
        );
        let recorded_shape = [
            "Quick safety check: Is this a project you created or one you trust? (Like your",
            "own code, a well-known open source project, or work from your team). If not,",
            "take a moment to review what's in this folder first.",
            "",
            "Claude Code'll be able to read, edit, and execute files here.",
            "",
            "Security guide",
            "",
            "❯  No, exit",
            "   Yes, I trust this folder",
            "",
            "Enter to confirm · Esc to cancel",
        ]
        .join("\n");
        assert_eq!(classify(&recorded_shape, id), StartupPrompt::FolderTrust);
        for text in [
            "Quick safety check: Is this a project you created or one you trust?\n❯ No, exit\n  Yes, I trust this folder",
            "trust this folder",
            "Do you trust the files in this folder?\n❯ 1. Yes, proceed",
            "An arbitrary question?\n❯ 1. Yes, proceed\n  2. No, exit",
            "No conversation found with session ID 11111111-1111-4111-8111-111111111111",
        ] {
            assert_eq!(classify(text, id), StartupPrompt::Unclassified, "{text}");
        }
        assert_eq!(
            classify(&format!("No conversation found with session ID {id}"), id),
            StartupPrompt::OriginalResumeRejected
        );
        for text in [
            format!("Quoted No conversation found with session ID: {id}"),
            format!("No conversation found with session ID: {id}extra"),
        ] {
            assert_eq!(classify(&text, id), StartupPrompt::Unclassified);
        }
        assert_eq!(classify(&"x".repeat(16385), id), StartupPrompt::Unavailable);
    }
}
