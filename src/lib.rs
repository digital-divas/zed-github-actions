use std::{env, fs};

use zed_extension_api::{
    self as zed, process, serde_json, settings::LspSettings, LanguageServerId, Result, Worktree,
};

const PACKAGE_NAME: &str = "@actions/languageserver";
const SERVER_PATH: &str = "node_modules/@actions/languageserver/bin/actions-languageserver";

struct GitHubActionsExtension {
    did_find_server: bool,
}

impl GitHubActionsExtension {
    fn server_exists() -> bool {
        fs::metadata(SERVER_PATH).is_ok_and(|stat| stat.is_file())
    }

    fn server_script_path(&mut self, language_server_id: &LanguageServerId) -> Result<String> {
        let server_exists = Self::server_exists();
        if self.did_find_server && server_exists {
            return Ok(SERVER_PATH.to_string());
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let version = zed::npm_package_latest_version(PACKAGE_NAME)?;

        if !server_exists
            || zed::npm_package_installed_version(PACKAGE_NAME)?.as_ref() != Some(&version)
        {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            let result = zed::npm_install_package(PACKAGE_NAME, &version);
            match result {
                Ok(()) if Self::server_exists() => {}
                Ok(()) => Err(format!("installed package '{PACKAGE_NAME}' did not contain expected path '{SERVER_PATH}'"))?,
                // offline or registry down: keep the version already installed
                Err(error) if !server_exists => Err(error)?,
                Err(_) => {}
            }
        }

        self.did_find_server = true;
        Ok(SERVER_PATH.to_string())
    }
}

/// Runs a command found on the worktree's PATH and returns its trimmed stdout when it succeeds
fn run(worktree: &Worktree, program: &str, args: &[&str]) -> Option<String> {
    let binary = worktree.which(program)?;
    let output = process::Command::new(binary)
        .args(args.iter().copied())
        .envs(worktree.shell_env())
        .output()
        .ok()?;
    if output.status != Some(0) {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|out| out.trim().to_string())
}

/// Token for the GitHub API: GH_TOKEN / GITHUB_TOKEN from the shell, or the GitHub CLI login
fn github_token(worktree: &Worktree) -> Option<String> {
    worktree
        .shell_env()
        .into_iter()
        .find(|(key, value)| (key == "GH_TOKEN" || key == "GITHUB_TOKEN") && !value.is_empty())
        .map(|(_, value)| value)
        .or_else(|| run(worktree, "gh", &["auth", "token"]))
        .filter(|token| !token.is_empty())
}

/// "owner/name" of a github.com remote, over SSH or HTTPS
fn github_slug(remote: &str) -> Option<String> {
    let path = remote
        .strip_prefix("git@github.com:")
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))
        .or_else(|| remote.strip_prefix("https://github.com/"))
        .or_else(|| remote.split_once("@github.com/").map(|(_, path)| path))?;
    let slug = path.trim_end_matches('/').trim_end_matches(".git");
    (slug.split('/').count() == 2).then(|| slug.to_string())
}

fn file_uri(path: &str) -> String {
    let mut uri = String::from("file://");
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// Repository context the language server needs to fetch the secrets, variables and environments
fn repository_context(worktree: &Worktree) -> Option<serde_json::Value> {
    let root = worktree.root_path();
    let remote = run(
        worktree,
        "git",
        &["-C", &root, "remote", "get-url", "origin"],
    )?;
    let slug = github_slug(&remote)?;
    let repo: serde_json::Value =
        serde_json::from_str(&run(worktree, "gh", &["api", &format!("repos/{slug}")])?).ok()?;

    Some(serde_json::json!({
        "id": repo["id"].as_u64()?,
        "owner": repo["owner"]["login"].as_str()?,
        "name": repo["name"].as_str()?,
        "organizationOwned": repo["owner"]["type"].as_str() == Some("Organization"),
        "workspaceUri": file_uri(&root),
    }))
}

/// Recursively overrides `base` with the user's `overrides`
fn merge(base: &mut serde_json::Value, overrides: serde_json::Value) {
    match (base, overrides) {
        (serde_json::Value::Object(base), serde_json::Value::Object(overrides)) => {
            for (key, value) in overrides {
                merge(base.entry(key).or_insert(serde_json::Value::Null), value);
            }
        }
        (base, overrides) => *base = overrides,
    }
}

impl zed::Extension for GitHubActionsExtension {
    fn new() -> Self {
        Self {
            did_find_server: false,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<zed::Command> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)?;
        if let Some(binary) = settings
            .binary
            .and_then(|binary| binary.path.map(|path| (path, binary.arguments)))
        {
            return Ok(zed::Command {
                command: binary.0,
                args: binary.1.unwrap_or_else(|| vec!["--stdio".to_string()]),
                env: Default::default(),
            });
        }

        let server_path = self.server_script_path(language_server_id)?;
        Ok(zed::Command {
            command: zed::node_binary_path()?,
            args: vec![
                env::current_dir()
                    .unwrap()
                    .join(&server_path)
                    .to_string_lossy()
                    .to_string(),
                "--stdio".to_string(),
            ],
            env: Default::default(),
        })
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let mut options = serde_json::json!({});

        // without a token or a github.com origin the server still runs, only without the github.com validations
        if let Some(token) = github_token(worktree) {
            options["sessionToken"] = serde_json::Value::String(token);
            if let Some(repo) = repository_context(worktree) {
                options["repos"] = serde_json::json!([repo]);
            }
        }

        if let Some(overrides) =
            LspSettings::for_worktree(language_server_id.as_ref(), worktree)?.initialization_options
        {
            merge(&mut options, overrides);
        }
        Ok(Some(options))
    }
}

zed::register_extension!(GitHubActionsExtension);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_remotes() {
        for remote in [
            "git@github.com:Sentinel-X/smart-agent.git",
            "ssh://git@github.com/Sentinel-X/smart-agent.git",
            "https://github.com/Sentinel-X/smart-agent",
            "https://user:token@github.com/Sentinel-X/smart-agent.git/",
        ] {
            assert_eq!(
                github_slug(remote).as_deref(),
                Some("Sentinel-X/smart-agent"),
                "{remote}"
            );
        }
        assert_eq!(github_slug("git@gitlab.com:group/project.git"), None);
    }

    #[test]
    fn encodes_workspace_uri_like_zed() {
        assert_eq!(
            file_uri("/Users/me/My Projects/repo"),
            "file:///Users/me/My%20Projects/repo"
        );
    }

    #[test]
    fn user_options_override_detected_ones() {
        let mut options = serde_json::json!({ "sessionToken": "detected", "repos": [] });
        merge(
            &mut options,
            serde_json::json!({ "sessionToken": "mine", "logLevel": 1 }),
        );
        assert_eq!(
            options,
            serde_json::json!({ "sessionToken": "mine", "repos": [], "logLevel": 1 })
        );
    }
}
