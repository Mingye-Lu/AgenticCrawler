use std::io::Write;
use std::path::PathBuf;

use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use runtime::config_home_dir;

use crate::ModelSubcommand;

struct ModelInfo {
    display_name: &'static str,
    filename: &'static str,
    size_display: &'static str,
    url: &'static str,
}

const MODELS: &[ModelInfo] = &[
    ModelInfo {
        display_name: "tiny",
        filename: "ggml-tiny.en.bin",
        size_display: "~75MB",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
    },
    ModelInfo {
        display_name: "small",
        filename: "ggml-small.en.bin",
        size_display: "~466MB",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
    },
    ModelInfo {
        display_name: "large-turbo",
        filename: "ggml-large-v3-turbo-q5_0.bin",
        size_display: "~547MB",
        url:
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
    },
];

fn models_dir() -> PathBuf {
    config_home_dir().join("models")
}

pub async fn run_model_command(
    subcommand: ModelSubcommand,
) -> Result<(), Box<dyn std::error::Error>> {
    match subcommand {
        ModelSubcommand::Path => {
            println!("{}", models_dir().display());
        }
        ModelSubcommand::List => {
            let dir = models_dir();
            println!("{:<15} {:<10} {:<10}", "MODEL", "SIZE", "STATUS");
            println!("{}", "-".repeat(38));
            for model in MODELS {
                let path = dir.join(model.filename);
                let status = if path.exists() {
                    "downloaded"
                } else {
                    "not found"
                };
                println!(
                    "{:<15} {:<10} {}",
                    model.display_name, model.size_display, status
                );
            }
            println!();
            println!("Download with: acrawl model download <name>");
        }
        ModelSubcommand::Download { ref size } => {
            download_model(size).await?;
        }
    }
    Ok(())
}

async fn download_model(size: &str) -> Result<(), Box<dyn std::error::Error>> {
    let model = MODELS
        .iter()
        .find(|m| m.display_name == size)
        .ok_or_else(|| {
            format!("Unknown model size '{size}'. Available: tiny, small, large-turbo")
        })?;

    let dir = models_dir();
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(model.filename);
    let part = dest.with_extension("part");

    if dest.exists() {
        println!(
            "Model '{}' already downloaded at {}",
            model.display_name,
            dest.display()
        );
        return Ok(());
    }

    println!(
        "Downloading {} ({})...",
        model.display_name, model.size_display
    );
    println!("  URL: {}", model.url);
    println!("  Destination: {}", dest.display());

    let client = reqwest::Client::builder().user_agent("acrawl").build()?;

    let response = client.get(model.url).send().await?.error_for_status()?;
    let total = response.content_length();

    let pb = total.map(|total| {
        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("=>-"),
        );
        pb
    });

    let mut file = std::fs::File::create(&part)?;
    let mut downloaded = 0u64;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        downloaded += chunk.len() as u64;
        file.write_all(&chunk)?;
        if let Some(ref pb) = pb {
            pb.set_position(downloaded);
        }
    }
    drop(file);

    if let Some(pb) = pb {
        pb.finish_with_message("Download complete");
    }

    std::fs::rename(&part, &dest)?;

    println!("Downloaded {} to {}", model.display_name, dest.display());
    Ok(())
}
