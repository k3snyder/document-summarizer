use std::{
    collections::HashSet,
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitStatus,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

pub fn suppress_command_window(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

pub fn suppress_tokio_command_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

pub async fn run_cli_command(
    mut command: Command,
    stdin_text: &str,
    context: &str,
    timeout_seconds: u64,
    label: &str,
) -> Result<CommandOutput, String> {
    suppress_tokio_command_window(&mut command);
    prepend_executable_dir_to_path(&mut command);
    command.kill_on_drop(true);
    let mut child = command.spawn().map_err(|err| {
        format!(
            "could not start {label}: {err}; {context}; {}",
            unavailable_output("process did not start")
        )
    })?;

    let mut stdin = child.stdin.take().ok_or_else(|| {
        format!(
            "{label} stdin unavailable; {context}; {}",
            unavailable_output("stdin unavailable")
        )
    })?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        format!(
            "{label} stdout unavailable; {context}; {}",
            unavailable_output("stdout unavailable")
        )
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        format!(
            "{label} stderr unavailable; {context}; {}",
            unavailable_output("stderr unavailable")
        )
    })?;

    let stdin_text = stdin_text.to_string();
    let writer = tokio::spawn(async move {
        stdin.write_all(stdin_text.as_bytes()).await?;
        stdin.shutdown().await
    });
    let stdout_reader = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let stderr_reader = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });

    let status = tokio::select! {
        result = child.wait() => result.map_err(|err| {
            format!(
                "{label} failed while waiting for output: {err}; {context}; {}",
                unavailable_output("wait failed")
            )
        })?,
        _ = sleep(Duration::from_secs(timeout_seconds)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(format!(
                "{label} timed out; {context}; {}",
                unavailable_output("process timed out")
            ));
        }
    };

    let stdout_bytes = stdout_reader
        .await
        .map_err(|err| {
            format!(
                "{label} stdout reader failed: {err}; {context}; {}",
                unavailable_output("stdout reader failed")
            )
        })?
        .map_err(|err| {
            format!(
                "{label} stdout read failed: {err}; {context}; {}",
                unavailable_output("stdout read failed")
            )
        })?;
    let stderr_bytes = stderr_reader
        .await
        .map_err(|err| {
            format!(
                "{label} stderr reader failed: {err}; {context}; {}",
                unavailable_output("stderr reader failed")
            )
        })?
        .map_err(|err| {
            format!(
                "{label} stderr read failed: {err}; {context}; {}",
                unavailable_output("stderr read failed")
            )
        })?;
    let output = CommandOutput {
        stdout: String::from_utf8_lossy(&stdout_bytes).trim().to_string(),
        stderr: String::from_utf8_lossy(&stderr_bytes).trim().to_string(),
    };

    let write_result = writer.await.map_err(|err| {
        format!(
            "{label} stdin writer failed: {err}; {context}; {}",
            command_output(&output)
        )
    })?;

    if !status.success() {
        return Err(format!(
            "{label} exited with {status}; {context}; {}{}",
            command_output(&output),
            write_error_suffix(&write_result)
        ));
    }
    write_result.map_err(|err| {
        format!(
            "{label} stdin write failed: {err}; {context}; {}",
            command_output(&output)
        )
    })?;

    Ok(output)
}

pub async fn create_isolated_grok_home(parent_dir: &Path) -> Result<PathBuf, String> {
    let home_dir = parent_dir.join("grok-home");
    let grok_dir = home_dir.join(".grok");
    tokio::fs::create_dir_all(&grok_dir)
        .await
        .map_err(|err| format!("could not create isolated Grok home: {err}"))?;

    if let Some(source_home) = env::var_os("HOME") {
        copy_grok_auth(&grok_dir, Path::new(&source_home)).await?;
    }

    tokio::fs::write(grok_dir.join("config.toml"), ISOLATED_GROK_CONFIG)
        .await
        .map_err(|err| format!("could not write isolated Grok config: {err}"))?;
    tokio::fs::write(grok_dir.join("pager.toml"), "disable_plugins = true\n")
        .await
        .map_err(|err| format!("could not write isolated Grok pager config: {err}"))?;

    Ok(home_dir)
}

pub fn configure_isolated_grok_command(command: &mut Command, home_dir: &Path) {
    let grok_home = home_dir.join(".grok");
    command
        .env("HOME", home_dir)
        .env("GROK_HOME", grok_home)
        .env("GROK_MEMORY", "0")
        .env("GROK_SUBAGENTS", "0")
        .env("GROK_WEB_FETCH", "0")
        .env("GROK_CURSOR_SKILLS_ENABLED", "false")
        .env("GROK_CURSOR_RULES_ENABLED", "false")
        .env("GROK_CURSOR_AGENTS_ENABLED", "false")
        .env("GROK_CURSOR_MCPS_ENABLED", "false")
        .env("GROK_CURSOR_HOOKS_ENABLED", "false")
        .env("GROK_CLAUDE_SKILLS_ENABLED", "false")
        .env("GROK_CLAUDE_RULES_ENABLED", "false")
        .env("GROK_CLAUDE_AGENTS_ENABLED", "false")
        .env("GROK_CLAUDE_MCPS_ENABLED", "false")
        .env("GROK_CLAUDE_HOOKS_ENABLED", "false")
        .env("CMUX_GROK_HOOKS_DISABLED", "1");
}

const ISOLATED_GROK_CONFIG: &str = r#"[compat.cursor]
skills = false
rules = false
agents = false
mcps = false
hooks = false

[compat.claude]
skills = false
rules = false
agents = false
mcps = false
hooks = false

[plugins]
paths = []
disabled = []

[memory]
enabled = false
"#;

async fn copy_grok_auth(grok_dir: &Path, source_home: &Path) -> Result<(), String> {
    let source_auth = source_home.join(".grok").join("auth.json");
    if !source_auth.is_file() {
        return Ok(());
    }

    tokio::fs::copy(&source_auth, grok_dir.join("auth.json"))
        .await
        .map(|_| ())
        .map_err(|err| format!("could not copy Grok auth cache: {err}"))
}

fn prepend_executable_dir_to_path(command: &mut Command) {
    let Some(parent) = Path::new(command.as_std().get_program()).parent() else {
        return;
    };
    if parent.as_os_str().is_empty() {
        return;
    }

    let mut paths = vec![parent.to_path_buf()];
    if let Some(existing_path) = command_path_env(command).or_else(|| env::var_os("PATH")) {
        paths.extend(env::split_paths(&existing_path));
    }

    if let Ok(path) = env::join_paths(paths) {
        command.env("PATH", path);
    }
}

pub fn resolve_cli_executable(executable: &str) -> Option<PathBuf> {
    resolve_cli_executable_with_extra_dirs(executable, &[])
}

pub fn resolve_cli_executable_with_extra_dirs(
    executable: &str,
    extra_dirs: &[PathBuf],
) -> Option<PathBuf> {
    let executable = executable.trim();
    if executable.is_empty() {
        return None;
    }

    let path = Path::new(executable);
    if path.components().count() > 1 || path.is_absolute() {
        return executable_path_candidates(path)
            .into_iter()
            .find(|candidate| candidate.is_file());
    }

    executable_search_dirs(extra_dirs)
        .into_iter()
        .flat_map(|directory| {
            executable_path_candidates(Path::new(executable))
                .into_iter()
                .map(move |candidate| directory.join(candidate))
        })
        .find(|candidate| candidate.is_file())
}

pub fn cli_search_path_with_extra_dirs(extra_dirs: &[PathBuf]) -> Option<OsString> {
    env::join_paths(executable_search_dirs(extra_dirs)).ok()
}

pub fn resolve_soffice() -> Option<PathBuf> {
    env::var_os("SOFFICE_BIN")
        .or_else(|| env::var_os("LIBREOFFICE_BIN"))
        .map(PathBuf::from)
        .and_then(|path| {
            executable_path_candidates(&path)
                .into_iter()
                .find(|candidate| candidate.is_file())
        })
        .or_else(|| resolve_cli_executable("soffice"))
        .or_else(|| resolve_cli_executable("libreoffice"))
        .or_else(|| {
            standard_soffice_paths()
                .into_iter()
                .flat_map(|path| executable_path_candidates(&path))
                .find(|candidate| candidate.is_file())
        })
}

fn executable_path_candidates(path: &Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut candidates = vec![path.to_path_buf()];
        if path.extension().is_none() {
            for extension in windows_executable_extensions() {
                candidates.push(path.with_extension(extension.trim_start_matches('.')));
            }
        }
        candidates
    }

    #[cfg(not(windows))]
    {
        vec![path.to_path_buf()]
    }
}

fn executable_search_dirs(extra_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(paths) = env::var_os("PATH") {
        directories.extend(env::split_paths(&paths));
    }

    directories.extend(extra_dirs.iter().cloned());
    directories.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
        PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS"),
    ]);

    if let Some(home) = home_dir() {
        directories.push(home.join(".cargo/bin"));
        directories.push(home.join(".local/bin"));
        directories.push(home.join(".grok/bin"));
        directories.push(home.join(".nvm/current/bin"));
        let node_versions = home.join(".nvm/versions/node");
        if let Ok(entries) = std::fs::read_dir(node_versions) {
            let mut node_bins: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path().join("bin"))
                .filter(|path| path.is_dir())
                .collect();
            node_bins.sort();
            node_bins.reverse();
            directories.extend(node_bins);
        }
    }

    #[cfg(windows)]
    {
        directories.extend(env_path("APPDATA").map(|path| path.join("npm")));
        directories.extend(env_path("NVM_HOME"));
        directories.extend(env_path("NVM_SYMLINK"));
        directories.extend(env_path("LOCALAPPDATA").map(|path| path.join("Microsoft/WindowsApps")));
        directories
            .extend(env_path("LOCALAPPDATA").map(|path| path.join("Programs/LibreOffice/program")));
        directories.extend(env_path("ProgramFiles").map(|path| path.join("nodejs")));
        directories.extend(env_path("ProgramFiles(x86)").map(|path| path.join("nodejs")));
        directories.extend(env_path("ProgramFiles").map(|path| path.join("LibreOffice/program")));
        directories
            .extend(env_path("ProgramFiles(x86)").map(|path| path.join("LibreOffice/program")));
    }

    let mut seen = HashSet::new();
    directories
        .into_iter()
        .filter(|directory| seen.insert(directory_dedupe_key(directory)))
        .collect()
}

fn directory_dedupe_key(directory: &Path) -> String {
    let key = directory.display().to_string();
    if cfg!(windows) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

fn standard_soffice_paths() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut paths = vec![
            PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS/soffice"),
            PathBuf::from("/opt/homebrew/bin/soffice"),
            PathBuf::from("/usr/local/bin/soffice"),
        ];
        paths.extend(env_path("ProgramFiles").map(|path| path.join("LibreOffice/program/soffice")));
        paths.extend(
            env_path("ProgramFiles(x86)").map(|path| path.join("LibreOffice/program/soffice")),
        );
        paths.extend(
            env_path("LOCALAPPDATA").map(|path| path.join("Programs/LibreOffice/program/soffice")),
        );
        paths
    }

    #[cfg(not(windows))]
    {
        vec![
            PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS/soffice"),
            PathBuf::from("/opt/homebrew/bin/soffice"),
            PathBuf::from("/usr/local/bin/soffice"),
        ]
    }
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(windows)]
fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from)
}

#[cfg(windows)]
fn windows_executable_extensions() -> Vec<String> {
    let pathext = env::var_os("PATHEXT")
        .map(|value| {
            env::split_paths(&value)
                .filter_map(|path| path.as_os_str().to_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut extensions = if pathext.is_empty() {
        vec![
            ".exe".to_string(),
            ".cmd".to_string(),
            ".bat".to_string(),
            ".com".to_string(),
        ]
    } else {
        pathext
    };
    for fallback in [".exe", ".cmd", ".bat", ".com"] {
        if !extensions
            .iter()
            .any(|extension| extension.eq_ignore_ascii_case(fallback))
        {
            extensions.push(fallback.to_string());
        }
    }
    extensions
}

fn command_path_env(command: &Command) -> Option<OsString> {
    command
        .as_std()
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value.map(OsString::from))
}

pub fn cli_command_context(
    label: &str,
    executable: &str,
    args: &[String],
    timeout_seconds: u64,
) -> String {
    format!(
        "{label} executable={executable}; args={}; timeout_seconds={timeout_seconds}",
        serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string())
    )
}

pub fn command_status_message(
    label: &str,
    status: ExitStatus,
    context: &str,
    output: &CommandOutput,
) -> String {
    format!(
        "{label} exited with {status}; {context}; {}",
        command_output(output)
    )
}

pub fn command_output(output: &CommandOutput) -> String {
    format!(
        "stdout={}; stderr={}",
        output.stdout.trim(),
        output.stderr.trim()
    )
}

pub fn unavailable_output(reason: &str) -> String {
    format!("stdout=<unavailable: {reason}>; stderr=<unavailable: {reason}>")
}

pub fn parse_codex_jsonl(stdout: &str) -> String {
    let mut content = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if event["type"] == "item.completed" {
            let item = &event["item"];
            if item["type"] == "agent_message" {
                if let Some(text) = item["text"].as_str() {
                    content.push(text.to_string());
                }
            }
        } else if event["type"] == "message" {
            let message = &event["message"];
            if message["role"] == "assistant" {
                if let Some(text) = message["content"].as_str() {
                    content.push(text.to_string());
                } else if let Some(blocks) = message["content"].as_array() {
                    for block in blocks {
                        if let Some(text) = block.as_str() {
                            content.push(text.to_string());
                        } else if block["type"] == "text" {
                            if let Some(text) = block["text"].as_str() {
                                content.push(text.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    if content.is_empty() {
        stdout.trim().to_string()
    } else {
        content.join("\n").trim().to_string()
    }
}

pub fn parse_grok_json(stdout: &str) -> String {
    let trimmed = stdout.trim();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return trimmed.to_string();
    };
    value["text"]
        .as_str()
        .map(|text| text.trim().to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

fn write_error_suffix(result: &std::io::Result<()>) -> String {
    match result {
        Ok(()) => String::new(),
        Err(err) => format!("; stdin write failed: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cli_command_context, configure_isolated_grok_command, copy_grok_auth, parse_codex_jsonl,
        parse_grok_json, resolve_cli_executable_with_extra_dirs, run_cli_command,
        ISOLATED_GROK_CONFIG,
    };
    use std::collections::HashMap;
    #[cfg(unix)]
    use std::process::Command as StdCommand;
    use std::process::Stdio;
    use std::time::Instant;
    use tokio::process::Command;
    #[cfg(unix)]
    use tokio::time::{sleep, Duration};

    #[test]
    fn parses_codex_jsonl_assistant_messages() {
        let stdout = r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#;
        assert_eq!(parse_codex_jsonl(stdout), "hello");
    }

    #[test]
    fn parses_grok_json_text_field() {
        assert_eq!(parse_grok_json(r#"{"text":" hello "}"#), "hello");
    }

    #[tokio::test]
    async fn timeout_kills_slow_child() {
        let (mut command, executable, args) = slow_command();
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let context = cli_command_context("test CLI", executable, &args, 1);
        let started = Instant::now();

        let error = run_cli_command(command, "", &context, 1, "test CLI")
            .await
            .unwrap_err();

        assert!(started.elapsed().as_secs() < 5);
        assert!(error.contains("test CLI timed out"));
    }

    #[test]
    fn resolves_cli_executable_from_extra_directory() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let executable_path = fake_executable_path(&bin, "fake-cli");
        write_fake_executable(&executable_path);

        let resolved =
            resolve_cli_executable_with_extra_dirs("fake-cli", std::slice::from_ref(&bin)).unwrap();

        assert_same_executable_path(&resolved, &executable_path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn absolute_node_shim_finds_sibling_interpreter() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let fake_node = bin.join("fake-node");
        let shim = bin.join("fake-codex");
        std::fs::write(
            &fake_node,
            "#!/bin/sh\ncat >/dev/null\necho sibling-node-ok\n",
        )
        .unwrap();
        std::fs::write(&shim, "#!/usr/bin/env fake-node\n").unwrap();
        std::fs::set_permissions(&fake_node, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut command = Command::new(&shim);
        command
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let context = cli_command_context(
            "test CLI",
            &shim.display().to_string(),
            &Vec::<String>::new(),
            5,
        );

        let output = run_cli_command(command, "hello", &context, 5, "test CLI")
            .await
            .unwrap();

        assert_eq!(output.stdout, "sibling-node-ok");
    }

    #[tokio::test]
    async fn copies_only_grok_auth_cache_into_isolated_home() {
        let source = tempfile::tempdir().unwrap();
        let source_grok = source.path().join(".grok");
        std::fs::create_dir(&source_grok).unwrap();
        std::fs::write(source_grok.join("auth.json"), "{\"token\":\"cached\"}").unwrap();
        std::fs::write(source_grok.join("config.toml"), "[plugins]\n").unwrap();
        let target = tempfile::tempdir().unwrap();

        copy_grok_auth(target.path(), source.path()).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(target.path().join("auth.json")).unwrap(),
            "{\"token\":\"cached\"}"
        );
        assert!(!target.path().join("config.toml").exists());
    }

    #[test]
    fn isolated_grok_command_disables_user_context_sources() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let mut command = Command::new("grok");

        configure_isolated_grok_command(&mut command, &home);

        let envs = command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                Some((
                    key.to_string_lossy().into_owned(),
                    value?.to_string_lossy().into_owned(),
                ))
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(envs["HOME"], home.display().to_string());
        assert_eq!(envs["GROK_HOME"], home.join(".grok").display().to_string());
        assert_eq!(envs["GROK_MEMORY"], "0");
        assert_eq!(envs["GROK_CLAUDE_MCPS_ENABLED"], "false");
        assert_eq!(envs["GROK_CURSOR_MCPS_ENABLED"], "false");
        assert_eq!(envs["CMUX_GROK_HOOKS_DISABLED"], "1");
        assert!(ISOLATED_GROK_CONFIG.contains("[compat.claude]"));
        assert!(ISOLATED_GROK_CONFIG.contains("[plugins]"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_command_future_kills_child() {
        let temp = tempfile::tempdir().unwrap();
        let pid_path = temp.path().join("child.pid");
        let mut command = Command::new("sh");
        command
            .args(["-c", "echo $$ > \"$PID_FILE\"; sleep 30"])
            .env("PID_FILE", &pid_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let context = cli_command_context(
            "test CLI",
            "sh",
            &["-c".into(), "echo $$ > \"$PID_FILE\"; sleep 30".into()],
            60,
        );

        let handle =
            tokio::spawn(
                async move { run_cli_command(command, "", &context, 60, "test CLI").await },
            );
        let pid = wait_for_pid(&pid_path).await;
        handle.abort();
        let _ = handle.await;

        for _ in 0..50 {
            if !process_exists(pid) {
                return;
            }
            sleep(Duration::from_millis(20)).await;
        }
        panic!("child process {pid} still exists after command future was dropped");
    }

    #[cfg(unix)]
    async fn wait_for_pid(pid_path: &std::path::Path) -> u32 {
        for _ in 0..50 {
            if let Ok(contents) = std::fs::read_to_string(pid_path) {
                if let Ok(pid) = contents.trim().parse() {
                    return pid;
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
        panic!("child did not write pid file");
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        StdCommand::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    fn slow_command() -> (Command, &'static str, Vec<String>) {
        let args = vec!["-c".to_string(), "sleep 10".to_string()];
        let mut command = Command::new("sh");
        command.args(&args);
        (command, "sh", args)
    }

    #[cfg(windows)]
    fn slow_command() -> (Command, &'static str, Vec<String>) {
        let args = vec!["/C".to_string(), "ping -n 10 127.0.0.1 > NUL".to_string()];
        let mut command = Command::new("cmd");
        command.args(&args);
        (command, "cmd", args)
    }

    fn fake_executable_path(bin: &std::path::Path, name: &str) -> std::path::PathBuf {
        #[cfg(windows)]
        {
            bin.join(format!("{name}.cmd"))
        }
        #[cfg(not(windows))]
        {
            bin.join(name)
        }
    }

    fn write_fake_executable(path: &std::path::Path) {
        #[cfg(windows)]
        {
            std::fs::write(path, "@echo off\r\nexit /b 0\r\n").unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn assert_same_executable_path(left: &std::path::Path, right: &std::path::Path) {
        #[cfg(windows)]
        {
            assert_eq!(
                left.display().to_string().to_ascii_lowercase(),
                right.display().to_string().to_ascii_lowercase()
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(left, right);
        }
    }
}
