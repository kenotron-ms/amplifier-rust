    //! amplifier-module-hooks-status-context — mirrors Python hooks-status-context.
    //!
    //! Fires on ProviderRequest, injects an <env> block + git status as a
    //! SystemPromptAddendum. The session never sees stale date/directory info.

    use amplifier_module_orchestrator_loop_streaming::{Hook, HookContext, HookEvent, HookResult};
    use std::path::{Path, PathBuf};

    pub struct StatusContextHook {
        vault_path: PathBuf,
        session_id: String,
    }

    impl StatusContextHook {
        pub fn new(vault_path: PathBuf) -> Self {
            // Generate a stable session ID for this run
            let session_id = uuid::Uuid::new_v4().to_string();
            Self { vault_path, session_id }
        }
    }

    #[async_trait::async_trait]
    impl Hook for StatusContextHook {
        fn events(&self) -> &[HookEvent] {
            &[HookEvent::ProviderRequest]
        }

        async fn handle(&self, ctx: &HookContext) -> HookResult {
            if ctx.event != HookEvent::ProviderRequest {
                return HookResult::Continue;
            }

            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        
            // Platform — resolved at compile time, correct on Android
            let platform = if cfg!(target_os = "android") {
                "android"
            } else if cfg!(target_os = "macos") {
                "darwin"
            } else if cfg!(target_os = "linux") {
                "linux"
            } else {
                "unknown"
            };

            let os_version = if cfg!(target_os = "android") {
                "Android".to_string()
            } else {
                std::env::consts::ARCH.to_string()
            };

            let working_dir = self.vault_path.to_string_lossy().to_string();
            let is_git = self.vault_path.join(".git").exists();

            let env_block = format!(
                "<env>\nWorking directory: {working_dir}\nSession ID: {sid}\nIs sub-session: No\nIs directory a git repo: {git}\nPlatform: {platform}\nOS Version: {os_version}\nToday\'s date: {now}\n</env>",
                working_dir = working_dir,
                sid = self.session_id,
                git = if is_git { "Yes" } else { "No" },
                platform = platform,
                os_version = os_version,
                now = now,
            );

            let git_block = if is_git {
                git_status_block(&self.vault_path)
            } else {
                String::new()
            };

            let full = if git_block.is_empty() {
                env_block
            } else {
                format!("{env_block}\n\n{git_block}")
            };

            HookResult::SystemPromptAddendum(full)
        }
    }

    /// Try to get git status — first via subprocess, then by reading .git files directly.
    fn git_status_block(vault_path: &Path) -> String {
        // Try subprocess first (available on macOS/Linux dev machines, usually not on Android)
        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", &vault_path.to_string_lossy(), "log", "--oneline", "-5"])
            .output()
        {
            if out.status.success() {
                let log = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let branch = git_current_branch_cmd(vault_path).unwrap_or_else(|| "unknown".to_string());
                let status_line = git_status_line_cmd(vault_path);
                return format!(
                    "gitStatus: This is the git status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation.\nCurrent branch: {branch}\n\nMain branch (you will usually use this for PRs): main\n\nStatus:\n{status_line}\n\nRecent commits:\n{log}",
                    branch = branch,
                    status_line = status_line,
                    log = log,
                );
            }
        }

        // Fallback: read .git/HEAD directly (works on Android without git CLI)
        let head_path = vault_path.join(".git").join("HEAD");
        if let Ok(content) = std::fs::read_to_string(&head_path) {
            let branch = if let Some(b) = content.trim().strip_prefix("ref: refs/heads/") {
                b.to_string()
            } else {
                content.trim().chars().take(8).collect()
            };
            return format!(
                "gitStatus: Snapshot at conversation start.\nCurrent branch: {branch}\n\nMain branch (you will usually use this for PRs): main\n\nStatus:\n(git CLI unavailable on this platform)",
                branch = branch,
            );
        }

        String::new()
    }

    fn git_current_branch_cmd(vault_path: &Path) -> Option<String> {
        let out = std::process::Command::new("git")
            .args(["-C", &vault_path.to_string_lossy(), "rev-parse", "--abbrev-ref", "HEAD"])
            .output().ok()?;
        if out.status.success() {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            None
        }
    }

    fn git_status_line_cmd(vault_path: &Path) -> String {
        let out = std::process::Command::new("git")
            .args(["-C", &vault_path.to_string_lossy(), "status", "--short"])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s.is_empty() { "Working directory clean".to_string() } else { s }
            }
            _ => "Unknown".to_string(),
        }
    }
    