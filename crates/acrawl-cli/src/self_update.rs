use std::env;
use std::fs;
use std::path::PathBuf;

use runtime::{check_for_update_force, config_home_dir};

const REPO: &str = "Mingye-Lu/AgenticCrawler";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn run_self_update() -> Result<(), Box<dyn std::error::Error>> {
    println!("Checking for updates...");

    let update_info = check_for_update_force().await;
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

    println!("Downloading {artifact_name}...");
    let binary_bytes = client
        .get(&binary_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    let checksums_text = client
        .get(&checksums_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    println!("Verifying checksum...");
    verify_checksum(&binary_bytes, &checksums_text, artifact_name)?;

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

async fn install_cloakbrowser_if_needed() {
    let cloakbrowser_dir = config_home_dir().join("node_modules").join("cloakbrowser");
    if cloakbrowser_dir.exists() {
        return;
    }

    println!("Installing CloakBrowser...");
    let config_home = config_home_dir();
    let npm_result = tokio::process::Command::new("npm")
        .args(["install", "--prefix"])
        .arg(&config_home)
        .arg("cloakbrowser")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;

    if npm_result.is_ok_and(|s| s.success()) {
        println!("CloakBrowser installed.");
    }
}
