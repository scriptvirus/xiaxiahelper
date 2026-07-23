#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use aes_gcm::aead::{Aead, AeadCore, OsRng};
use aes_gcm::{Aes256Gcm, KeyInit};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::Local;
use flate2::write::GzEncoder;
use flate2::Compression;
use git2::{
    Cred, DiffOptions, FetchOptions, IndexAddOption, PushOptions, RemoteCallbacks, Repository,
    Signature, Status, StatusOptions, Tree,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use ssh2::Session;
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

#[derive(Clone)]
struct PublishConfig {
    git_remote: String,
    remote_host: String,
    remote_user: String,
    remote_port: u16,
    remote_project_dir: String,
    remote_service: String,
    remote_domain: String,
    ssh_key: Option<PathBuf>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectStatus {
    branch: String,
    status: String,
    diff_stat: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitInfo {
    short: String,
    full: String,
    subject: String,
}

#[derive(Serialize, Clone)]
struct LogPayload {
    line: String,
}

#[derive(Serialize)]
struct RunResult {
    code: i32,
}

#[derive(Default, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SecretConfig {
    github_token: String,
    ssh_key: String,
    server_password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SecretStatus {
    exists: bool,
    github_token: bool,
    ssh_key: bool,
    server_password: bool,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupStatus {
    complete: bool,
}

#[derive(Serialize, Deserialize)]
struct EncryptedPayload {
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QuickCheckResult {
    ok: bool,
    lines: Vec<String>,
}

fn project_root(project_path: &str) -> Result<PathBuf, String> {
    let root = PathBuf::from(project_path);
    if !root.exists() {
        return Err(format!("项目目录不存在：{}", project_path));
    }
    if !root.join("虾虾发布助手").join("xiaxia_config.json").exists() {
        return Err("项目目录里没有找到 虾虾发布助手/xiaxia_config.json".into());
    }
    Ok(root)
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME").or_else(|_| env::var("USERPROFILE")) {
            return PathBuf::from(home).join(stripped);
        }
    }
    PathBuf::from(path)
}

fn app_config_dir() -> Result<PathBuf, String> {
    if cfg!(target_os = "windows") {
        if let Ok(appdata) = env::var("APPDATA") {
            return Ok(PathBuf::from(appdata).join("xiaxia-publish-assistant"));
        }
    }
    if cfg!(target_os = "macos") {
        if let Ok(home) = env::var("HOME") {
            return Ok(PathBuf::from(home).join("Library/Application Support/xiaxia-publish-assistant"));
        }
    }
    if let Ok(home) = env::var("HOME") {
        return Ok(PathBuf::from(home).join(".config/xiaxia-publish-assistant"));
    }
    Err("无法确定当前用户配置目录".into())
}

fn secrets_path() -> Result<PathBuf, String> {
    Ok(app_config_dir()?.join("secrets.enc.json"))
}

fn setup_complete_path() -> Result<PathBuf, String> {
    Ok(app_config_dir()?.join(".setup_complete"))
}

fn machine_key() -> [u8; 32] {
    let mut material = String::from("xiaxia-publish-assistant-local-secrets-v1|");
    material.push_str(&env::var("USER").or_else(|_| env::var("USERNAME")).unwrap_or_default());
    material.push('|');
    material.push_str(&env::var("HOME").or_else(|_| env::var("USERPROFILE")).unwrap_or_default());
    let digest = Sha256::digest(material.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}

fn encrypt_secrets(secrets: &SecretConfig) -> Result<EncryptedPayload, String> {
    let key = machine_key();
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|error| format!("初始化加密器失败：{error}"))?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let plaintext = serde_json::to_vec(secrets).map_err(|error| format!("序列化本机配置失败：{error}"))?;
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_ref())
        .map_err(|error| format!("加密本机配置失败：{error}"))?;
    Ok(EncryptedPayload {
        nonce: BASE64.encode(nonce),
        ciphertext: BASE64.encode(ciphertext),
    })
}

fn decrypt_secrets(payload: &EncryptedPayload) -> Result<SecretConfig, String> {
    let key = machine_key();
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|error| format!("初始化解密器失败：{error}"))?;
    let nonce = BASE64.decode(&payload.nonce).map_err(|error| format!("解析 nonce 失败：{error}"))?;
    let ciphertext = BASE64
        .decode(&payload.ciphertext)
        .map_err(|error| format!("解析密文失败：{error}"))?;
    let plaintext = cipher
        .decrypt(nonce.as_slice().into(), ciphertext.as_ref())
        .map_err(|_| "解密本机配置失败：可能不是当前电脑保存的配置。".to_string())?;
    serde_json::from_slice(&plaintext).map_err(|error| format!("解析本机配置失败：{error}"))
}

fn load_secrets() -> Result<SecretConfig, String> {
    let path = secrets_path()?;
    if !path.exists() {
        return Ok(SecretConfig::default());
    }
    let text = std::fs::read_to_string(&path).map_err(|error| format!("读取本机加密配置失败：{}\n{}", path.display(), error))?;
    let payload: EncryptedPayload = serde_json::from_str(&text).map_err(|error| format!("本机加密配置格式错误：{error}"))?;
    decrypt_secrets(&payload)
}

fn save_secrets_file(secrets: &SecretConfig) -> Result<(), String> {
    let path = secrets_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建本机配置目录失败：{error}"))?;
    }
    let payload = encrypt_secrets(secrets)?;
    let text = serde_json::to_string_pretty(&payload).map_err(|error| format!("序列化加密配置失败：{error}"))?;
    std::fs::write(&path, text).map_err(|error| format!("写入本机加密配置失败：{}\n{}", path.display(), error))
}

#[tauri::command]
fn saved_secrets_status() -> Result<SecretStatus, String> {
    let path = secrets_path()?;
    let secrets = load_secrets()?;
    Ok(SecretStatus {
        exists: path.exists(),
        github_token: !secrets.github_token.trim().is_empty(),
        ssh_key: !secrets.ssh_key.trim().is_empty(),
        server_password: !secrets.server_password.trim().is_empty(),
        path: path.display().to_string(),
    })
}

#[tauri::command]
fn save_secret_config(github_token: String, ssh_key: String, server_password: String) -> Result<SecretStatus, String> {
    let existing = load_secrets().unwrap_or_default();
    let secrets = SecretConfig {
        github_token: if github_token.trim().is_empty() {
            existing.github_token
        } else {
            github_token.trim().to_string()
        },
        ssh_key: if ssh_key.trim().is_empty() {
            existing.ssh_key
        } else {
            ssh_key.trim().to_string()
        },
        server_password: if server_password.trim().is_empty() {
            existing.server_password
        } else {
            server_password
        },
    };
    save_secrets_file(&secrets)?;
    saved_secrets_status()
}

#[tauri::command]
fn check_setup_complete() -> Result<SetupStatus, String> {
    let path = setup_complete_path()?;
    Ok(SetupStatus { complete: path.exists() })
}

#[tauri::command]
fn complete_setup() -> Result<(), String> {
    let path = setup_complete_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建本机配置目录失败：{error}"))?;
    }
    std::fs::write(&path, "1").map_err(|error| format!("写入设置完成标记失败：{error}"))?;
    Ok(())
}

const CLONE_URL: &str = "https://github.com/scriptvirus/liukexiaxia.git";

#[tauri::command]
fn clone_project(app: AppHandle, target_dir: String) -> Result<String, String> {
    let path = PathBuf::from(&target_dir);

    if path.exists() {
        let is_empty = std::fs::read_dir(&path)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !is_empty {
            return Err("目标目录不为空，请选择一个空目录。".into());
        }
    }

    emit_line(&app, format!("==> 正在从 GitHub 克隆项目..."));
    emit_line(&app, format!("git clone {} {}", CLONE_URL, target_dir));

    let repo = git2::build::RepoBuilder::new()
        .clone(CLONE_URL, &path)
        .map_err(|error| format!("克隆项目失败：{error}"))?;

    let branch = repo
        .head()
        .ok()
        .and_then(|head| head.shorthand().map(str::to_string))
        .unwrap_or_else(|| "main".to_string());

    emit_line(&app, format!("OK  项目克隆完成，当前分支：{branch}"));
    Ok(target_dir)
}

fn config_value(value: &Value, key: &str, default: &str) -> String {
    value
        .get(key)
        .and_then(|item| item.as_str())
        .unwrap_or(default)
        .to_string()
}

fn load_config(root: &Path) -> Result<PublishConfig, String> {
    let config_path = root.join("虾虾发布助手").join("xiaxia_config.json");
    let text = std::fs::read_to_string(&config_path)
        .map_err(|error| format!("读取配置失败：{}\n{}", config_path.display(), error))?;
    let value: Value = serde_json::from_str(&text).map_err(|error| format!("配置 JSON 格式错误：{error}"))?;
    let ssh_key = value
        .get("ssh_key")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(expand_home);
    Ok(PublishConfig {
        git_remote: config_value(&value, "git_remote", "origin"),
        remote_host: config_value(&value, "remote_host", "liukexin.antarcticheifer.top"),
        remote_user: config_value(&value, "remote_user", "root"),
        remote_port: value
            .get("remote_port")
            .and_then(|item| item.as_u64())
            .unwrap_or(22) as u16,
        remote_project_dir: config_value(&value, "remote_project_dir", "/opt/liuk"),
        remote_service: config_value(&value, "remote_service", "liuk"),
        remote_domain: config_value(&value, "remote_domain", "liukexin.antarcticheifer.top"),
        ssh_key,
    })
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn effective_github_token(input: &str, secrets: &SecretConfig) -> Option<String> {
    non_empty(input).or_else(|| non_empty(&secrets.github_token))
}

fn effective_ssh_key(input: &str, secrets: &SecretConfig, config: &PublishConfig) -> Option<PathBuf> {
    non_empty(input)
        .or_else(|| non_empty(&secrets.ssh_key))
        .map(|path| expand_home(&path))
        .or_else(|| config.ssh_key.clone())
}

fn effective_server_password(input: &str, secrets: &SecretConfig) -> Option<String> {
    if input.is_empty() {
        if secrets.server_password.is_empty() {
            None
        } else {
            Some(secrets.server_password.clone())
        }
    } else {
        Some(input.to_string())
    }
}

fn merged_path() -> String {
    let mut entries: Vec<String> = Vec::new();
    if cfg!(target_os = "windows") {
        // nvm-windows default path
        if let Ok(appdata) = env::var("APPDATA") {
            let nvm_home = PathBuf::from(&appdata).join("nvm");
            if nvm_home.exists() {
                entries.push(nvm_home.to_string_lossy().to_string());
            }
        }
        // Common Windows tool locations
        for dir in [
            "C:\\Python313",
            "C:\\Python312",
            "C:\\Python311",
            "C:\\Python310",
            "C:\\Program Files\\Git\\cmd",
        ] {
            if Path::new(dir).exists() {
                entries.push(dir.to_string());
            }
        }
        if let Ok(current) = env::var("PATH") {
            entries.push(current);
        }
        return entries.join(";");
    }
    if let Ok(home) = env::var("HOME") {
        entries.push(format!("{home}/.cargo/bin"));
        let nvm_versions = PathBuf::from(&home).join(".nvm/versions/node");
        if let Ok(nodes) = std::fs::read_dir(nvm_versions) {
            let mut versions: Vec<PathBuf> = nodes.filter_map(|entry| entry.ok().map(|item| item.path())).collect();
            versions.sort();
            versions.reverse();
            if let Some(path) = versions.first() {
                entries.push(path.join("bin").to_string_lossy().to_string());
            }
        }
    }
    entries.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ]
        .iter()
        .map(|item| item.to_string()),
    );
    if let Ok(current) = env::var("PATH") {
        entries.push(current);
    }
    entries.join(":")
}

fn git_repo(root: &Path) -> Result<Repository, String> {
    Repository::open(root).map_err(|error| format!("打开 Git 仓库失败：{error}"))
}

fn current_branch(repo: &Repository) -> Result<String, String> {
    let head = repo.head().map_err(|error| format!("读取当前分支失败：{error}"))?;
    head.shorthand()
        .map(str::to_string)
        .ok_or("当前 Git 不在普通分支上，无法自动发布。".into())
}

fn short_oid(oid: git2::Oid) -> String {
    oid.to_string().chars().take(7).collect()
}

fn status_pair(status: Status) -> &'static str {
    if status == Status::WT_NEW {
        return "??";
    }
    let index = if status.contains(Status::INDEX_NEW) {
        "A"
    } else if status.contains(Status::INDEX_MODIFIED) {
        "M"
    } else if status.contains(Status::INDEX_DELETED) {
        "D"
    } else if status.contains(Status::INDEX_RENAMED) {
        "R"
    } else if status.contains(Status::INDEX_TYPECHANGE) {
        "T"
    } else {
        " "
    };
    let worktree = if status.contains(Status::WT_NEW) {
        "?"
    } else if status.contains(Status::WT_MODIFIED) {
        "M"
    } else if status.contains(Status::WT_DELETED) {
        "D"
    } else if status.contains(Status::WT_RENAMED) {
        "R"
    } else if status.contains(Status::WT_TYPECHANGE) {
        "T"
    } else {
        " "
    };
    match (index, worktree) {
        (" ", " ") => "  ",
        ("A", " ") => "A ",
        ("M", " ") => "M ",
        ("D", " ") => "D ",
        ("R", " ") => "R ",
        ("T", " ") => "T ",
        (" ", "M") => " M",
        (" ", "D") => " D",
        (" ", "?") => "??",
        (" ", "R") => " R",
        (" ", "T") => " T",
        ("A", "M") => "AM",
        ("M", "M") => "MM",
        ("D", "M") => "DM",
        ("A", "D") => "AD",
        ("M", "D") => "MD",
        _ => "??",
    }
}

fn git_status_short(repo: &Repository) -> Result<String, String> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true);
    let statuses = repo.statuses(Some(&mut opts)).map_err(|error| format!("读取 Git 状态失败：{error}"))?;
    let mut lines = Vec::new();
    for entry in statuses.iter() {
        let path = entry.path().unwrap_or("");
        if path.is_empty() {
            continue;
        }
        lines.push(format!("{} {}", status_pair(entry.status()), path));
    }
    Ok(lines.join("\n"))
}

fn git_diff_stat(repo: &Repository) -> Result<String, String> {
    let mut output = Vec::new();
    let mut opts = DiffOptions::new();
    let unstaged = repo
        .diff_index_to_workdir(None, Some(&mut opts))
        .map_err(|error| format!("读取未暂存 diff 失败：{error}"))?;
    let unstaged_stats = unstaged.stats().map_err(|error| format!("生成未暂存 diff 统计失败：{error}"))?;
    if unstaged_stats.files_changed() > 0 {
        output.push(format!(
            "未暂存：{} files changed, {} insertions(+), {} deletions(-)",
            unstaged_stats.files_changed(),
            unstaged_stats.insertions(),
            unstaged_stats.deletions()
        ));
    }

    if let Ok(head) = repo.head().and_then(|head| head.peel_to_tree()) {
        let index = repo.index().map_err(|error| format!("读取 Git index 失败：{error}"))?;
        let staged = repo
            .diff_tree_to_index(Some(&head), Some(&index), None)
            .map_err(|error| format!("读取已暂存 diff 失败：{error}"))?;
        let staged_stats = staged.stats().map_err(|error| format!("生成已暂存 diff 统计失败：{error}"))?;
        if staged_stats.files_changed() > 0 {
            output.push(format!(
                "已暂存：{} files changed, {} insertions(+), {} deletions(-)",
                staged_stats.files_changed(),
                staged_stats.insertions(),
                staged_stats.deletions()
            ));
        }
    }
    Ok(output.join("\n"))
}

fn git_callbacks(github_token: Option<String>) -> RemoteCallbacks<'static> {
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(move |_url, username_from_url, _allowed_types| {
        if let Some(token) = github_token.as_ref().filter(|token| !token.trim().is_empty()) {
            Cred::userpass_plaintext(username_from_url.unwrap_or("x-access-token"), token)
        } else {
            Cred::default()
        }
    });
    callbacks
}

fn fetch_remote(app: &AppHandle, repo: &Repository, remote_name: &str, branch: &str, token: Option<String>) -> Result<(), String> {
    emit_line(app, format!("git2 fetch {remote_name} {branch}"));
    let mut remote = repo.find_remote(remote_name).map_err(|error| format!("找不到 Git 远程 {remote_name}：{error}"))?;
    let mut options = FetchOptions::new();
    options.remote_callbacks(git_callbacks(token));
    remote
        .fetch(&[branch], Some(&mut options), None)
        .map_err(|error| format!("Git fetch 失败：{error}"))
}

fn push_remote(app: &AppHandle, repo: &Repository, remote_name: &str, branch: &str, token: Option<String>) -> Result<(), String> {
    emit_line(app, format!("git2 push {remote_name} {branch}"));
    let mut remote = repo.find_remote(remote_name).map_err(|error| format!("找不到 Git 远程 {remote_name}：{error}"))?;
    let mut options = PushOptions::new();
    options.remote_callbacks(git_callbacks(token));
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    remote
        .push(&[refspec.as_str()], Some(&mut options))
        .map_err(|error| format!("Git push 失败：{error}"))
}

#[tauri::command]
fn project_status(project_path: String) -> Result<ProjectStatus, String> {
    let root = project_root(&project_path)?;
    let repo = git_repo(&root)?;
    let branch = current_branch(&repo).unwrap_or_default();
    let status = git_status_short(&repo).unwrap_or_default();
    let diff_stat = git_diff_stat(&repo).unwrap_or_default();
    Ok(ProjectStatus {
        branch,
        status,
        diff_stat,
    })
}

#[tauri::command]
fn recent_commits(project_path: String) -> Result<Vec<CommitInfo>, String> {
    let root = project_root(&project_path)?;
    let repo = git_repo(&root)?;
    let mut walk = repo.revwalk().map_err(|error| format!("读取 commit 历史失败：{error}"))?;
    walk.push_head().map_err(|error| format!("读取 HEAD 失败：{error}"))?;
    let mut commits = Vec::new();
    for oid in walk.take(5) {
        let oid = oid.map_err(|error| format!("读取 commit id 失败：{error}"))?;
        let commit = repo.find_commit(oid).map_err(|error| format!("读取 commit 失败：{error}"))?;
        commits.push(CommitInfo {
            short: short_oid(oid),
            full: oid.to_string(),
            subject: commit.summary().unwrap_or("(无提交说明)").to_string(),
        });
    }
    Ok(commits)
}

fn emit_line(app: &AppHandle, line: impl Into<String>) {
    let _ = app.emit("assistant-log", LogPayload { line: line.into() });
}

fn command_exists(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .env("PATH", merged_path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn python_command() -> (String, Vec<String>) {
    if cfg!(target_os = "windows") {
        if command_exists("py", &["-3", "--version"]) {
            return ("py".into(), vec!["-3".into(), "-u".into()]);
        }
        return ("python".into(), vec!["-u".into()]);
    }
    if command_exists("python3", &["--version"]) {
        return ("python3".into(), vec!["-u".into()]);
    }
    ("python".into(), vec!["-u".into()])
}

fn ok_line(lines: &mut Vec<String>, message: &str) {
    lines.push(format!("OK  {message}"));
}

fn fail_line(lines: &mut Vec<String>, message: &str) {
    lines.push(format!("FAIL  {message}"));
}

fn check_file(lines: &mut Vec<String>, root: &Path, path: &str) -> bool {
    if root.join(path).exists() {
        ok_line(lines, &format!("{path} 存在"));
        true
    } else {
        fail_line(lines, &format!("{path} 不存在"));
        false
    }
}

fn required_files() -> [&'static str; 16] {
    [
        "index.html",
        "page1.html",
        "page2.html",
        "page3.html",
        "page4.html",
        "page5.html",
        "page6.html",
        "page7.html",
        "page8.html",
        "page9.html",
        "page10.html",
        "backend/app.py",
        "backend/requirements.txt",
        "assets/js/state-api.js",
        "虾虾发布助手/xiaxia_publish.py",
        "虾虾发布助手/xiaxia_config.json",
    ]
}

#[tauri::command]
fn quick_check(project_path: String) -> Result<QuickCheckResult, String> {
    let root = project_root(&project_path)?;
    let config = load_config(&root)?;
    let secrets = load_secrets().unwrap_or_default();
    let repo = git_repo(&root)?;
    let mut lines = vec!["==> 快速检查".to_string()];
    let mut ok = true;

    ok_line(&mut lines, "内置 Git 可用");

    for path in required_files() {
        ok = check_file(&mut lines, &root, path) && ok;
    }

    let key = effective_ssh_key("", &secrets, &config);
    let password = effective_server_password("", &secrets);
    if let Some(key) = key {
        if key.exists() {
            ok_line(&mut lines, &format!("SSH key 存在：{}", key.display()));
        } else {
            fail_line(&mut lines, &format!("SSH key 不存在：{}", key.display()));
            ok = false;
        }
    } else if password.is_some() {
        ok_line(&mut lines, "已保存服务器密码，可用于 SSH 登录");
    } else {
        fail_line(&mut lines, "未配置 SSH key 或服务器密码；原生发布必须至少配置一种服务器登录方式。");
        ok = false;
    }

    if !secrets.github_token.trim().is_empty() {
        ok_line(&mut lines, "已保存 GitHub Token");
    } else {
        lines.push("WARN 未保存 GitHub Token；发布时需要临时输入 Token。".into());
    }

    match git_status_short(&repo) {
        Ok(status) if status.is_empty() => ok_line(&mut lines, "Git 工作区干净"),
        Ok(status) => {
            lines.push("WARN Git 工作区有变动：".into());
            lines.extend(status.lines().map(|line| format!("  {line}")));
        }
        Err(error) => {
            fail_line(&mut lines, &format!("Git 状态读取失败：{error}"));
            ok = false;
        }
    }

    if ok {
        ok_line(&mut lines, "快速检查通过");
    } else {
        fail_line(&mut lines, "快速检查未通过");
    }

    Ok(QuickCheckResult { ok, lines })
}

fn matches_excluded(relative: &str) -> bool {
    let normalized = relative.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.iter().any(|part| matches!(*part, ".git" | ".venv" | "venv" | "__pycache__" | ".pytest_cache")) {
        return true;
    }
    if normalized == ".DS_Store" || normalized.ends_with("/.DS_Store") {
        return true;
    }
    if normalized == ".env" || normalized.ends_with("/.env") {
        return true;
    }
    if normalized.starts_with("data/backups/") {
        return true;
    }
    if normalized.starts_with("data/") && (normalized.ends_with(".db") || normalized.contains(".db-")) {
        return true;
    }
    [".xlsx", ".xls", ".xlsm"].iter().any(|suffix| normalized.ends_with(suffix))
}

fn create_package(app: &AppHandle, root: &Path, version: &str) -> Result<PathBuf, String> {
    emit_line(app, "==> 打包代码");
    let archive = env::temp_dir().join(format!("xiaxia-liuk-{version}.tar.gz"));
    if archive.exists() {
        std::fs::remove_file(&archive).map_err(|error| format!("删除旧部署包失败：{error}"))?;
    }
    let file = File::create(&archive).map_err(|error| format!("创建部署包失败：{error}"))?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut tar = tar::Builder::new(encoder);
    for entry in WalkDir::new(root).into_iter().filter_map(|item| item.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("计算相对路径失败：{error}"))?
            .to_string_lossy()
            .replace('\\', "/");
        if matches_excluded(&relative) {
            continue;
        }
        tar.append_path_with_name(path, &relative)
            .map_err(|error| format!("写入部署包失败：{relative}\n{error}"))?;
    }
    tar.finish().map_err(|error| format!("完成部署包失败：{error}"))?;
    emit_line(app, format!("OK  部署包已创建：{}", archive.display()));
    Ok(archive)
}

fn create_git_archive(app: &AppHandle, root: &Path, commit: &str, version: &str) -> Result<PathBuf, String> {
    emit_line(app, format!("==> 按 commit 打包：{commit}"));
    let repo = git_repo(root)?;
    let object = repo.revparse_single(commit).map_err(|error| format!("解析 commit 失败：{error}"))?;
    let commit = object.peel_to_commit().map_err(|error| format!("目标不是 commit：{error}"))?;
    let tree = commit.tree().map_err(|error| format!("读取 commit 文件树失败：{error}"))?;
    let archive = env::temp_dir().join(format!("xiaxia-liuk-{version}.tar.gz"));
    if archive.exists() {
        std::fs::remove_file(&archive).map_err(|error| format!("删除旧部署包失败：{error}"))?;
    }
    let file = File::create(&archive).map_err(|error| format!("创建部署包失败：{error}"))?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut tar = tar::Builder::new(encoder);
    append_git_tree(&repo, &tree, "", &mut tar)?;
    tar.finish().map_err(|error| format!("完成部署包失败：{error}"))?;
    emit_line(app, format!("OK  部署包已创建：{}", archive.display()));
    Ok(archive)
}

fn append_git_tree<W: std::io::Write>(
    repo: &Repository,
    tree: &Tree<'_>,
    prefix: &str,
    tar: &mut tar::Builder<W>,
) -> Result<(), String> {
    for entry in tree.iter() {
        let name = entry.name().ok_or("Git tree 包含非法文件名")?;
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        match entry.kind() {
            Some(git2::ObjectType::Tree) => {
                let subtree = repo.find_tree(entry.id()).map_err(|error| format!("读取目录失败：{path}\n{error}"))?;
                append_git_tree(repo, &subtree, &path, tar)?;
            }
            Some(git2::ObjectType::Blob) => {
                let blob = repo.find_blob(entry.id()).map_err(|error| format!("读取文件失败：{path}\n{error}"))?;
                let mut header = tar::Header::new_gnu();
                header.set_path(&path).map_err(|error| format!("设置归档路径失败：{path}\n{error}"))?;
                header.set_size(blob.size() as u64);
                let executable = entry.filemode() & 0o111 != 0;
                header.set_mode(if executable { 0o755 } else { 0o644 });
                header.set_cksum();
                tar.append(&header, Cursor::new(blob.content()))
                    .map_err(|error| format!("写入归档失败：{path}\n{error}"))?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn remote_script(config: &PublishConfig, version: &str, archive_name: &str, script_name: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

VERSION="{version}"
PROJECT_DIR="{project_dir}"
SERVICE="{service}"
DOMAIN="{domain}"
ARCHIVE="/tmp/{archive_name}"
BACKUP_ROOT="/opt/liuk_code_backups"
TMP_DIR="/tmp/liuk-publish-${{VERSION}}"

echo "==> 准备服务器目录"
mkdir -p "$PROJECT_DIR" "$BACKUP_ROOT"
rm -rf "$TMP_DIR"
mkdir -p "$TMP_DIR"

if [ -d "$PROJECT_DIR" ] && [ "$(find "$PROJECT_DIR" -mindepth 1 -maxdepth 1 2>/dev/null | head -n 1)" ]; then
  echo "==> 备份当前线上代码"
  tar --exclude='.venv' --exclude='data' --exclude='*.db' --exclude='*.db-*' \
    -czf "$BACKUP_ROOT/code-${{VERSION}}.tar.gz" -C "$PROJECT_DIR" .
fi

echo "==> 解压新版本"
tar -xzf "$ARCHIVE" -C "$TMP_DIR"
find "$TMP_DIR" -name '._*' -delete

echo "==> 覆盖代码文件，保留 .venv 和数据目录"
rm -rf "$PROJECT_DIR/assets" "$PROJECT_DIR/backend" "$PROJECT_DIR/scripts" "$PROJECT_DIR/docs" "$PROJECT_DIR/虾虾发布助手"
rm -f "$PROJECT_DIR/index.html" "$PROJECT_DIR/README.md" "$PROJECT_DIR/.gitignore" "$PROJECT_DIR"/page*.html
cp -a "$TMP_DIR"/. "$PROJECT_DIR"/

echo "==> 检查 Python 虚拟环境"
if [ ! -d "$PROJECT_DIR/.venv" ]; then
  if command -v python3.11 >/dev/null 2>&1; then
    python3.11 -m venv "$PROJECT_DIR/.venv"
  else
    python3 -m venv "$PROJECT_DIR/.venv"
  fi
fi

echo "==> 安装/更新后端依赖"
"$PROJECT_DIR/.venv/bin/pip" install -r "$PROJECT_DIR/backend/requirements.txt"

echo "==> 重启后端服务"
systemctl restart "$SERVICE"
sleep 2
systemctl is-active --quiet "$SERVICE"

echo "==> 检查并重载 Nginx"
nginx -t
systemctl reload nginx

echo "==> 服务器本机验证"
curl -sS http://127.0.0.1:8000/api/health
printf '\n'
curl -sS -H "Host: $DOMAIN" -o /dev/null -w "site %{{http_code}}\n" http://127.0.0.1/

echo "==> 清理临时文件"
rm -rf "$TMP_DIR" "$ARCHIVE" "/tmp/{script_name}"
echo "==> 发布完成：$VERSION"
"#,
        version = version,
        project_dir = config.remote_project_dir,
        service = config.remote_service,
        domain = config.remote_domain,
        archive_name = archive_name,
        script_name = script_name
    )
}

fn connect_ssh(app: &AppHandle, config: &PublishConfig, server_password: Option<&str>) -> Result<Session, String> {
    emit_line(
        app,
        format!(
            "==> 连接服务器 {}@{}:{}",
            config.remote_user, config.remote_host, config.remote_port
        ),
    );
    let tcp = TcpStream::connect((config.remote_host.as_str(), config.remote_port))
        .map_err(|error| format!("连接服务器失败：{error}"))?;
    let mut session = Session::new().map_err(|error| format!("创建 SSH 会话失败：{error}"))?;
    session.set_tcp_stream(tcp);
    session.handshake().map_err(|error| format!("SSH 握手失败：{error}"))?;

    // Display host key fingerprint for user verification
    if let Some((key, key_type)) = session.host_key() {
        let fingerprint: String = key.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(":");
        emit_line(app, format!("SSH 主机密钥 (type={key_type:?})：{fingerprint}"));
    }
    if let Some(key) = config.ssh_key.as_ref() {
        match session.userauth_pubkey_file(&config.remote_user, None, key, None) {
            Ok(_) => emit_line(app, "OK  SSH key 登录成功"),
            Err(error) => {
                if let Some(password) = server_password {
                    emit_line(app, format!("WARN SSH key 登录失败，尝试服务器密码：{error}"));
                    session
                        .userauth_password(&config.remote_user, password)
                        .map_err(|password_error| format!("SSH key 和服务器密码登录均失败：{}\n{}", error, password_error))?;
                } else {
                    return Err(format!("SSH key 登录失败：{}\n{}", key.display(), error));
                }
            }
        }
    } else if let Some(password) = server_password {
        session
            .userauth_password(&config.remote_user, password)
            .map_err(|error| format!("服务器密码登录失败：{error}"))?;
    } else {
        return Err("原生发布必须配置 SSH key 或服务器密码。".into());
    }
    if !session.authenticated() {
        return Err("SSH 登录失败：认证未通过".into());
    }
    emit_line(app, "OK  SSH 登录成功");
    Ok(session)
}

fn sftp_upload(app: &AppHandle, session: &Session, local: &Path, remote: &str) -> Result<(), String> {
    emit_line(app, format!("上传：{} -> {remote}", local.display()));
    let sftp = session.sftp().map_err(|error| format!("创建 SFTP 会话失败：{error}"))?;
    let mut remote_file = sftp.create(Path::new(remote)).map_err(|error| format!("创建远程文件失败：{remote}\n{error}"))?;
    let mut local_file = File::open(local).map_err(|error| format!("打开本地文件失败：{}\n{}", local.display(), error))?;
    std::io::copy(&mut local_file, &mut remote_file).map_err(|error| format!("上传文件失败：{error}"))?;
    emit_line(app, "OK  上传完成");
    Ok(())
}

fn exec_remote(app: &AppHandle, session: &Session, command: &str) -> Result<(), String> {
    emit_line(app, format!("远程执行：{command}"));
    let mut channel = session.channel_session().map_err(|error| format!("创建远程执行通道失败：{error}"))?;
    channel.exec(command).map_err(|error| format!("执行远程命令失败：{error}"))?;
    let mut output = String::new();
    channel.read_to_string(&mut output).map_err(|error| format!("读取远程输出失败：{error}"))?;
    for line in output.lines() {
        emit_line(app, line);
    }
    channel.wait_close().map_err(|error| format!("关闭远程通道失败：{error}"))?;
    let status = channel.exit_status().map_err(|error| format!("读取远程退出码失败：{error}"))?;
    if status != 0 {
        return Err(format!("远程命令失败，退出码：{status}"));
    }
    Ok(())
}

fn deploy_archive(
    app: &AppHandle,
    config: &PublishConfig,
    server_password: Option<&str>,
    version: &str,
    archive: &Path,
) -> Result<(), String> {
    emit_line(app, "==> 原生上传并部署服务器");
    let session = connect_ssh(app, config, server_password)?;
    let archive_name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("部署包文件名无效")?;
    let script_name = format!("xiaxia-native-deploy-{version}.sh");
    let script_path = env::temp_dir().join(&script_name);
    let script = remote_script(config, version, archive_name, &script_name);
    std::fs::write(&script_path, script).map_err(|error| format!("写入远程脚本临时文件失败：{error}"))?;
    sftp_upload(app, &session, archive, &format!("/tmp/{archive_name}"))?;
    sftp_upload(app, &session, &script_path, &format!("/tmp/{script_name}"))?;
    let result = exec_remote(app, &session, &format!("bash /tmp/{script_name}"));
    let _ = std::fs::remove_file(script_path);
    result?;
    emit_line(app, "OK  服务器部署完成");
    Ok(())
}

fn check_forbidden_index(repo: &Repository) -> Result<(), String> {
    let index = repo.index().map_err(|error| format!("读取 Git index 失败：{error}"))?;
    let bad: Vec<String> = index
        .iter()
        .filter_map(|entry| std::str::from_utf8(&entry.path).ok().map(str::to_string))
        .filter(|path| matches_excluded(path))
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!("以下敏感文件已被 Git 跟踪，请先移除：\n{}", bad.join("\n")))
    }
}

fn git_signature(repo: &Repository) -> Result<Signature<'_>, String> {
    repo.signature().or_else(|_| Signature::now("虾虾发布助手", "xiaxia-publish-assistant@example.local"))
        .map_err(|error| format!("创建 Git 提交作者失败：{error}"))
}

fn native_commit_and_push(
    app: &AppHandle,
    root: &Path,
    config: &PublishConfig,
    message: &str,
    github_token: Option<String>,
) -> Result<(), String> {
    emit_line(app, "==> Git 提交并推送");
    let repo = git_repo(root)?;
    let branch = current_branch(&repo)?;
    fetch_remote(app, &repo, &config.git_remote, &branch, github_token.clone())?;
    let remote_ref = format!("{}/{}", config.git_remote, branch);
    let head_ref = repo.head().map_err(|error| format!("读取 HEAD 失败：{error}"))?;
    let local_head = head_ref.target().ok_or("HEAD 没有指向 commit")?;
    let remote_full_ref = format!("refs/remotes/{remote_ref}");
    if let Ok(remote_reference) = repo.find_reference(&remote_full_ref) {
        let remote_head = remote_reference.target().ok_or("远程分支没有指向 commit")?;
        let merge_base = repo
            .merge_base(local_head, remote_head)
            .map_err(|error| format!("计算本地/远程共同祖先失败：{error}"))?;
        if local_head == remote_head {
            emit_line(app, "OK  本地和远程一致");
        } else if merge_base == remote_head {
            emit_line(app, "OK  本地包含远程最新提交，可以继续发布");
        } else if merge_base == local_head {
            return Err(format!(
                "GitHub 上的 {remote_ref} 比本地新。请先同步远程代码后再发布，避免覆盖别人改动。"
            ));
        } else {
            return Err(format!(
                "本地分支和 GitHub 的 {remote_ref} 已经分叉，存在冲突风险。请先人工处理 Git 冲突。"
            ));
        }
    } else {
        emit_line(app, format!("WARN 远程分支 {remote_ref} 不存在，将作为新分支推送"));
    }
    let status = git_status_short(&repo)?;
    if !status.trim().is_empty() {
        emit_line(app, "检测到代码变动：");
        for line in status.lines() {
            emit_line(app, format!("  {line}"));
        }
        let mut index = repo.index().map_err(|error| format!("读取 Git index 失败：{error}"))?;
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .map_err(|error| format!("暂存文件失败：{error}"))?;
        index.write().map_err(|error| format!("写入 Git index 失败：{error}"))?;
        check_forbidden_index(&repo)?;
        let tree_id = index.write_tree().map_err(|error| format!("写入 Git tree 失败：{error}"))?;
        let parent = repo
            .head()
            .and_then(|head| head.peel_to_commit())
            .map_err(|error| format!("读取当前 commit 失败：{error}"))?;
        if tree_id == parent.tree_id() {
            emit_line(app, "WARN 没有可提交内容，跳过 commit。");
        } else {
            let tree = repo.find_tree(tree_id).map_err(|error| format!("读取 Git tree 失败：{error}"))?;
            let signature = git_signature(&repo)?;
            repo.commit(Some("HEAD"), &signature, &signature, message, &tree, &[&parent])
                .map_err(|error| format!("创建 Git commit 失败：{error}"))?;
            emit_line(app, format!("OK  已提交：{message}"));
        }
    } else {
        emit_line(app, "OK  没有新的本地改动，跳过 commit");
    }
    push_remote(app, &repo, &config.git_remote, &branch, github_token)?;
    emit_line(app, format!("OK  已推送到 {}/{}", config.git_remote, branch));
    Ok(())
}

#[tauri::command]
fn native_deploy(
    app: AppHandle,
    project_path: String,
    message: String,
    github_token: String,
    ssh_key: String,
    server_password: String,
) -> Result<RunResult, String> {
    let root = project_root(&project_path)?;
    let mut config = load_config(&root)?;
    let secrets = load_secrets().unwrap_or_default();
    let github_token = effective_github_token(&github_token, &secrets)
        .ok_or("请先填写或保存 GitHub Token。去掉 Git for Windows 后，发布需要 Token 来 fetch/push GitHub。")?;
    config.ssh_key = effective_ssh_key(&ssh_key, &secrets, &config);
    let server_password = effective_server_password(&server_password, &secrets);
    emit_line(&app, "==> 原生发布开始");
    for path in required_files() {
        if !root.join(path).exists() {
            return Err(format!("缺少必要文件：{path}"));
        }
    }
    if let Some(key) = &config.ssh_key {
        if !key.exists() {
            return Err(format!("SSH key 不存在：{}", key.display()));
        }
    } else if server_password.is_none() {
        return Err("原生发布必须配置 SSH key 或服务器密码。".into());
    }
    native_commit_and_push(&app, &root, &config, &message, Some(github_token))?;
    let repo = git_repo(&root)?;
    let head = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|error| format!("读取提交后 HEAD 失败：{error}"))?;
    let version = format!("{}-{}", Local::now().format("%Y%m%d-%H%M%S"), short_oid(head.id()));
    let archive = create_package(&app, &root, &version)?;
    let result = deploy_archive(&app, &config, server_password.as_deref(), &version, &archive);
    let _ = std::fs::remove_file(&archive);
    result?;
    emit_line(&app, "原生发布执行成功。");
    Ok(RunResult { code: 0 })
}

#[tauri::command]
fn native_rollback(app: AppHandle, project_path: String, commit: String) -> Result<RunResult, String> {
    let root = project_root(&project_path)?;
    let mut config = load_config(&root)?;
    let secrets = load_secrets().unwrap_or_default();
    config.ssh_key = effective_ssh_key("", &secrets, &config);
    let server_password = effective_server_password("", &secrets);
    emit_line(&app, format!("==> 原生回滚到 commit：{commit}"));
    let repo = git_repo(&root)?;
    let object = repo.revparse_single(&commit).map_err(|error| format!("解析回滚 commit 失败：{error}"))?;
    let target = object.peel_to_commit().map_err(|error| format!("目标不是 commit：{error}"))?;
    let short = short_oid(target.id());
    let full = target.id().to_string();
    let version = format!("rollback-{}-{}", Local::now().format("%Y%m%d-%H%M%S"), short);
    let archive = create_git_archive(&app, &root, &full, &version)?;
    let result = deploy_archive(&app, &config, server_password.as_deref(), &version, &archive);
    let _ = std::fs::remove_file(&archive);
    result?;
    emit_line(&app, format!("原生回滚执行成功：{}", short));
    Ok(RunResult { code: 0 })
}

fn validate_assistant_args(action: &str, extra_args: &[String]) -> Result<(), String> {
    match action {
        "check" => {
            if extra_args.is_empty() {
                Ok(())
            } else {
                Err("检查命令不接受额外参数".into())
            }
        }
        "deploy" => {
            let mut index = 0;
            while index < extra_args.len() {
                match extra_args[index].as_str() {
                    "-y" | "--yes" => index += 1,
                    "-m" | "--message" => {
                        if index + 1 >= extra_args.len() {
                            return Err("发布说明参数缺少内容".into());
                        }
                        index += 2;
                    }
                    other => return Err(format!("发布命令不支持参数：{}", other)),
                }
            }
            Ok(())
        }
        "rollback" => {
            let mut index = 0;
            while index < extra_args.len() {
                match extra_args[index].as_str() {
                    "-y" | "--yes" => index += 1,
                    "--commit" => {
                        if index + 1 >= extra_args.len() {
                            return Err("回滚 commit 参数缺少内容".into());
                        }
                        index += 2;
                    }
                    other => return Err(format!("回滚命令不支持参数：{}", other)),
                }
            }
            Ok(())
        }
        _ => Err(format!("不支持的操作：{}", action)),
    }
}

#[tauri::command]
fn run_assistant(
    app: AppHandle,
    project_path: String,
    action: String,
    extra_args: Vec<String>,
) -> Result<RunResult, String> {
    let root = project_root(&project_path)?;
    validate_assistant_args(&action, &extra_args)?;
    let script = root.join("虾虾发布助手").join("xiaxia_publish.py");
    if !script.exists() {
        return Err("没有找到 Python 发布脚本。".into());
    }
    let (python, mut args) = python_command();

    args.push(script.to_string_lossy().to_string());
    args.push(action);
    args.extend(extra_args);

    emit_line(&app, format!("桌面端：启动 {}", args.join(" ")));

    let mut child = Command::new(&python)
        .args(&args)
        .current_dir(&root)
        .env("PATH", merged_path())
        .env("PYTHONUNBUFFERED", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动发布助手：{}", error))?;

    let stdout = child.stdout.take().ok_or("无法读取 stdout")?;
    let stderr = child.stderr.take().ok_or("无法读取 stderr")?;

    let app_for_stdout = app.clone();
    let stdout_thread = std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            emit_line(&app_for_stdout, line);
        }
    });

    let app_for_stderr = app.clone();
    let stderr_thread = std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            emit_line(&app_for_stderr, line);
        }
    });

    let status = child.wait().map_err(|error| format!("发布助手异常退出：{}", error))?;
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    let code = status.code().unwrap_or(1);
    emit_line(&app, format!("桌面端：进程退出码 {}", code));
    Ok(RunResult { code })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            project_status,
            recent_commits,
            saved_secrets_status,
            save_secret_config,
            check_setup_complete,
            complete_setup,
            clone_project,
            quick_check,
            native_deploy,
            native_rollback,
            run_assistant
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    run();
}
