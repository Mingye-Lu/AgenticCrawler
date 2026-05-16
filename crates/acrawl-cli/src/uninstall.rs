use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use runtime::config_home_dir;

pub fn run_uninstall(purge: bool) -> Result<(), Box<dyn std::error::Error>> {
    let config_home = config_home_dir();
    let current_exe = env::current_exe()?;

    println!("This will remove:");
    println!("  Binary:       {}", current_exe.display());
    let node_modules = config_home.join("node_modules");
    if node_modules.exists() {
        println!("  node_modules: {}", node_modules.display());
    }
    if purge {
        println!("  Settings:     {}", config_home.join("settings.json").display());
        println!("  Credentials:  {}", config_home.join("credentials.json").display());
        println!("  Sessions:     {}", config_home.join("sessions").display());
    }
    println!();

    print!("Uninstall acrawl? (y/N): ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if !matches!(input.trim(), "y" | "Y") {
        println!("Aborted.");
        return Ok(());
    }

    if node_modules.exists() {
        fs::remove_dir_all(&node_modules)?;
        println!("Removed node_modules.");
    }

    if purge {
        remove_file_if_exists(&config_home.join("settings.json"), "settings")?;
        remove_file_if_exists(&config_home.join("credentials.json"), "credentials")?;
        remove_dir_if_exists(&config_home.join("sessions"), "sessions")?;
    }

    remove_binary(&current_exe)?;

    #[cfg(target_os = "windows")]
    if let Some(bin_dir) = current_exe.parent() {
        remove_from_windows_path(bin_dir);
    }

    if let Some(bin_dir) = current_exe.parent() {
        let _ = fs::remove_dir(bin_dir);
    }
    if purge {
        match fs::remove_dir_all(&config_home) {
            Ok(()) => println!("Removed config directory: {}", config_home.display()),
            Err(_) => {
                let _ = fs::remove_dir(&config_home);
            }
        }
    } else {
        let _ = fs::remove_dir(&config_home);
    }

    println!("\nacrawl uninstalled successfully.");
    if cfg!(target_os = "windows") {
        println!("Restart your terminal for PATH changes to take effect.");
    }

    Ok(())
}

fn remove_binary(current_exe: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(target_os = "windows") {
        let old_path = current_exe.with_extension("exe.old");
        if fs::rename(current_exe, &old_path).is_ok() {
            let _ = fs::remove_file(&old_path);
            if old_path.exists() {
                println!(
                    "Binary renamed to {} — delete it after this process exits.",
                    old_path.display()
                );
            } else {
                println!("Removed binary: {}", current_exe.display());
            }
        } else {
            println!(
                "Warning: could not remove binary {} — delete it manually after this process exits.",
                current_exe.display()
            );
        }
    } else {
        match fs::remove_file(current_exe) {
            Ok(()) => println!("Removed binary: {}", current_exe.display()),
            Err(e) => println!(
                "Warning: could not remove binary {}: {e}",
                current_exe.display()
            ),
        }
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path, label: &str) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        fs::remove_file(path)?;
        println!("Removed {label}.");
    }
    Ok(())
}

fn remove_dir_if_exists(path: &Path, label: &str) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        fs::remove_dir_all(path)?;
        println!("Removed {label}.");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn remove_from_windows_path(bin_dir: &Path) {
    let bin_str = bin_dir.to_string_lossy();
    let script = format!(
        "$p = [Environment]::GetEnvironmentVariable('Path','User'); \
         $new = ($p -split ';' | Where-Object {{ $_ -ne '{bin_str}' }}) -join ';'; \
         [Environment]::SetEnvironmentVariable('Path',$new,'User')"
    );
    let ok = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        println!("Removed from PATH.");
    } else {
        println!("Warning: could not remove {bin_str} from PATH automatically.");
        println!("  Remove it manually from your user environment variables.");
    }
}
