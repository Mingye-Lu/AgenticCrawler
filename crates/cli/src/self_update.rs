use std::env;
use std::fs;
use std::path::PathBuf;
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

    let update_info = match update_info {
        Some(info) if info.is_outdated => info,
        Some(info) => {
            println!("Already up to date (v{}).", info.current_version);
            return Ok(());
        }
        None => {
            println!("Already up to date (v{CURRENT_VERSION}).");
            return Ok(());
        }
    };

    let version = &update_info.latest_version;
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

    install_cloakbrowser_if_needed().await;

    println!("Updated to v{version} successfully!");
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
    let config_home = config_home_dir();

    let pb = make_spinner("Updating CloakBrowser package...");
    let npm_result = tokio::process::Command::new("npm")
        .args(["install", "--prefix"])
        .arg(&config_home)
        .arg("cloakbrowser@latest")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    pb.finish_and_clear();

    if !npm_result.is_ok_and(|s| s.success()) {
        println!("WARNING: CloakBrowser package update failed.");
        return;
    }
    println!("CloakBrowser package updated.");

    let pb = make_spinner("Downloading browser binary...");
    let browser_dl = tokio::process::Command::new("npx")
        .args(["--prefix"])
        .arg(&config_home)
        .args(["cloakbrowser", "install"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
    pb.finish_and_clear();

    if browser_dl.is_ok_and(|s| s.success()) {
        println!("Browser binary ready.");
    } else {
        println!("WARNING: Browser binary download failed. It will be downloaded on first use.");
    }
}
