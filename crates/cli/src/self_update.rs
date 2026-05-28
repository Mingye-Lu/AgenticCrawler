use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};
use runtime::{check_for_update_force, config_home_dir};

const REPO: &str = "Mingye-Lu/AgenticCrawler";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn make_spinner(msg: impl Into<String>) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap(),
    );
    pb.set_message(msg.into());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

pub async fn run_self_update() -> Result<(), Box<dyn std::error::Error>> {
    let pb = make_spinner("Checking for updates...");
    let update_info = check_for_update_force().await;
    pb.finish_and_clear();

    let binary_updated = match update_info {
        Some(info) if info.is_outdated => {
            update_binary(&info.latest_version).await?;
            true
        }
        Some(info) => {
            println!("Already up to date (v{}).", info.current_version);
            false
        }
        None => {
            println!("Already up to date (v{CURRENT_VERSION}).");
            false
        }
    };

    install_cloakbrowser_if_needed().await;

    if binary_updated {
        println!("Update complete!");
    }
    Ok(())
}

async fn update_binary(version: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("Updating v{CURRENT_VERSION} -> v{version}...");

    let artifact_name = platform_artifact()?;
    let base_url = format!("https://github.com/{REPO}/releases/download/v{version}");
    let binary_url = format!("{base_url}/{artifact_name}");
    let checksums_url = format!("{base_url}/checksums.sha256");

    let client = reqwest::Client::builder().user_agent("acrawl").build()?;

    let pb = make_spinner(format!("Downloading {artifact_name}..."));
    let binary_bytes = download_tolerant(&client, &binary_url).await?;
    pb.finish_and_clear();
    println!(
        "Downloaded {artifact_name} ({} KB).",
        binary_bytes.len() / 1024
    );

    let pb = make_spinner("Downloading checksums...");
    let checksums_text = client
        .get(&checksums_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    pb.finish_and_clear();

    let pb = make_spinner("Verifying checksum...");
    verify_checksum(&binary_bytes, &checksums_text, artifact_name)?;
    pb.finish_and_clear();
    println!("Checksum verified.");

    let current_exe = env::current_exe()?;
    replace_binary(&current_exe, &binary_bytes)?;
    Ok(())
}

fn platform_artifact() -> Result<&'static str, Box<dyn std::error::Error>> {
    let artifact = match (env::consts::OS, env::consts::ARCH) {
        ("linux", "x86_64") => "acrawl-linux-x64",
        ("linux", "aarch64") => "acrawl-linux-arm64",
        ("macos", "x86_64") => "acrawl-macos-x64",
        ("macos", "aarch64") => "acrawl-macos-arm64",
        ("windows", "x86_64") => "acrawl-windows-x64.exe",
        (os, arch) => {
            return Err(format!("unsupported platform: {os}/{arch}").into());
        }
    };
    Ok(artifact)
}

fn verify_checksum(
    binary_bytes: &[u8],
    checksums_text: &str,
    artifact_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};

    let expected_hash = checksums_text
        .lines()
        .find(|line| line.contains(artifact_name))
        .and_then(|line| line.split_whitespace().next())
        .ok_or("artifact not found in checksums file")?;

    let mut hasher = Sha256::new();
    hasher.update(binary_bytes);
    let actual_hash = format!("{:x}", hasher.finalize());

    if actual_hash != expected_hash {
        return Err(
            format!("checksum mismatch: expected {expected_hash}, got {actual_hash}").into(),
        );
    }

    Ok(())
}

fn replace_binary(
    current_exe: &PathBuf,
    binary_bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = current_exe
        .parent()
        .ok_or("cannot determine executable directory")?;

    if cfg!(target_os = "windows") {
        let old_path = current_exe.with_extension("exe.old");
        let new_path = parent.join(".acrawl-update.exe");

        fs::write(&new_path, binary_bytes)?;

        if old_path.exists() {
            let _ = fs::remove_file(&old_path);
        }
        fs::rename(current_exe, &old_path)?;
        fs::rename(&new_path, current_exe)?;
        let _ = fs::remove_file(&old_path);
    } else {
        let temp_path = parent.join(".acrawl-update");
        fs::write(&temp_path, binary_bytes)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o755))?;
        }

        fs::rename(&temp_path, current_exe)?;
    }

    Ok(())
}

/// Downloads a URL using chunked streaming that tolerates servers closing
/// the connection without a TLS `close_notify` alert (common with Azure CDN
/// backing GitHub Releases). If all bytes indicated by `Content-Length` have
/// been received, the missing alert is ignored; otherwise the error propagates.
async fn download_tolerant(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    let content_length = response.content_length();

    let capacity = usize::try_from(content_length.unwrap_or(0)).unwrap_or(0);
    let mut buffer = Vec::with_capacity(capacity);
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => buffer.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(_) if content_length.is_some_and(|n| buffer.len() as u64 >= n) => break,
            Err(e) => return Err(e.into()),
        }
    }

    if let Some(expected) = content_length {
        if (buffer.len() as u64) < expected {
            return Err(format!(
                "incomplete download: got {} of {expected} bytes",
                buffer.len()
            )
            .into());
        }
    }

    Ok(buffer)
}

async fn install_cloakbrowser_if_needed() {
    let node_check = tokio::process::Command::new("node")
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await;

    let node_major = match node_check {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout);
            let version = version.trim().trim_start_matches('v');
            version
                .split('.')
                .next()
                .and_then(|s| s.parse::<u32>().ok())
        }
        _ => None,
    };

    match node_major {
        None => {
            println!(
                "Skipping CloakBrowser update: Node.js not found.\n  \
                 Install Node.js 20+ from https://nodejs.org/ for browser automation."
            );
            return;
        }
        Some(major) if major < 20 => {
            println!(
                "Skipping CloakBrowser update: Node.js 20+ required (found v{major}.x).\n  \
                 Upgrade from https://nodejs.org/"
            );
            return;
        }
        _ => {}
    }

    let config_home = config_home_dir();
    let _ = std::fs::create_dir_all(&config_home);

    if update_cloakbrowser_package(&config_home).await {
        download_browser_binary(&config_home).await;
    }
}

fn npm_command() -> tokio::process::Command {
    if cfg!(windows) {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.args(["/C", "npm"]);
        cmd
    } else {
        tokio::process::Command::new("npm")
    }
}

fn npx_command() -> tokio::process::Command {
    if cfg!(windows) {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.args(["/C", "npx"]);
        cmd
    } else {
        tokio::process::Command::new("npx")
    }
}

async fn update_cloakbrowser_package(config_home: &Path) -> bool {
    let pb = make_spinner("Updating CloakBrowser package...");
    let npm_timeout = Duration::from_mins(2);
    let npm_result = tokio::time::timeout(
        npm_timeout,
        npm_command()
            .args(["install", "--prefix"])
            .arg(config_home)
            .arg("cloakbrowser@latest")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await;
    pb.finish_and_clear();

    match npm_result {
        Ok(Ok(output)) if output.status.success() => {
            println!("CloakBrowser package updated.");
            true
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!(
                "WARNING: CloakBrowser package update failed (exit {}).",
                output.status
            );
            print_stderr_tail(&stderr);
            println!(
                "  Run manually: npm install --prefix \"{}\" cloakbrowser@latest",
                config_home.display()
            );
            false
        }
        Ok(Err(e)) => {
            println!("WARNING: Could not run npm: {e}");
            println!(
                "  Run manually: npm install --prefix \"{}\" cloakbrowser@latest",
                config_home.display()
            );
            false
        }
        Err(_) => {
            println!(
                "WARNING: CloakBrowser package update timed out after {}s.",
                npm_timeout.as_secs()
            );
            println!(
                "  Run manually: npm install --prefix \"{}\" cloakbrowser@latest",
                config_home.display()
            );
            false
        }
    }
}

async fn download_browser_binary(config_home: &Path) {
    let cloakbrowser_bin = config_home
        .join("node_modules")
        .join(".bin")
        .join(if cfg!(windows) {
            "cloakbrowser.cmd"
        } else {
            "cloakbrowser"
        });

    let pb = make_spinner("Downloading browser binary...");
    let dl_timeout = Duration::from_mins(5);

    let mut cmd = if cloakbrowser_bin.exists() {
        let mut c = tokio::process::Command::new(&cloakbrowser_bin);
        c.arg("install");
        c
    } else {
        let mut c = npx_command();
        c.args(["cloakbrowser", "install"]);
        c
    };
    cmd.env("NODE_PATH", config_home.join("node_modules"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());

    let browser_dl = tokio::time::timeout(dl_timeout, cmd.output()).await;
    pb.finish_and_clear();

    match browser_dl {
        Ok(Ok(output)) if output.status.success() => {
            println!("Browser binary ready.");
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!(
                "WARNING: Browser binary download failed (exit {}).",
                output.status
            );
            print_stderr_tail(&stderr);
            println!("  The browser will be downloaded automatically on first use.");
        }
        Ok(Err(e)) => {
            println!("WARNING: Could not download browser binary: {e}");
            println!("  The browser will be downloaded automatically on first use.");
        }
        Err(_) => {
            println!(
                "WARNING: Browser binary download timed out after {}s.",
                dl_timeout.as_secs()
            );
            println!("  The browser will be downloaded automatically on first use.");
        }
    }
}

fn print_stderr_tail(stderr: &str) {
    if !stderr.trim().is_empty() {
        let tail: Vec<&str> = stderr.lines().rev().take(5).collect();
        for line in tail.into_iter().rev() {
            println!("  {line}");
        }
    }
}
