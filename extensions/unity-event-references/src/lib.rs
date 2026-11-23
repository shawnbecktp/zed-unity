use zed_extension_api::{self as zed, LanguageServerId, Result, Worktree};

struct UnityEvents;

impl zed::Extension for UnityEvents {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<zed::Command> {
        // Zed places this extension's writable directory under extensions/work/<id>.
        let executable = if matches!(zed::current_platform().0, zed::Os::Windows) {
            "unity-event-lsp.exe"
        } else {
            "unity-event-lsp"
        };
        Ok(zed::Command {
            command: format!("../../installed/unity-event-references/{executable}"),
            args: vec![],
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(UnityEvents);
