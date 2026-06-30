use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ProcessingError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaMetadata {
    pub format: String,
    pub duration_seconds: f64,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub bitrate_kbps: Option<u32>,
    pub tags: HashMap<String, String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscribeOptions {
    pub language: Option<String>,
    pub timestamps: bool,
    pub model_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscribeOutput {
    pub metadata: MediaMetadata,
    pub transcript: String,
    pub segments: Option<Vec<Segment>>,
    pub duration_seconds: f64,
}

pub fn media_metadata(path: &Path) -> Result<MediaMetadata, ProcessingError> {
    let fs_metadata = std::fs::metadata(path)?;
    let size_bytes = fs_metadata.len();
    let format = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("unknown")
        .to_lowercase();

    #[cfg(not(feature = "transcription"))]
    {
        Ok(MediaMetadata {
            format,
            duration_seconds: 0.0,
            codec: None,
            sample_rate: None,
            channels: None,
            bitrate_kbps: None,
            tags: HashMap::new(),
            size_bytes,
        })
    }

    #[cfg(feature = "transcription")]
    {
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::probe::Hint;

        let src = std::fs::File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(src), Default::default());

        let mut hint = Hint::new();
        hint.with_extension(&format);

        let meta_opts = Default::default();
        let fmt_opts = Default::default();

        let probed = symphonia::default::get_probe().format(&hint, mss, &fmt_opts, &meta_opts);

        match probed {
            Ok(probed) => {
                let track = probed.format.default_track();
                let (duration_seconds, sample_rate, channels, codec, bitrate_kbps) =
                    track.map_or((0.0, None, None, None, None), |track| {
                        let params = &track.codec_params;
                        let sample_rate = params.sample_rate;
                        let channels = params.channels.map(|value| value.count() as u8);
                        let duration_seconds = params
                            .n_frames
                            .zip(params.time_base)
                            .map(|(frames, time_base)| time_base.calc_time(frames).seconds as f64)
                            .or_else(|| {
                                params
                                    .n_frames
                                    .zip(sample_rate)
                                    .map(|(frames, rate)| frames as f64 / rate as f64)
                            })
                            .unwrap_or(0.0);
                        let codec = match format!("{:?}", params.codec).as_str() {
                            "CODEC_TYPE_NULL" => None,
                            other => Some(other.to_string()),
                        };
                        let bitrate_kbps = params
                            .bits_per_coded_sample
                            .zip(sample_rate)
                            .zip(channels)
                            .map(|((bits_per_sample, rate), channels)| {
                                ((u64::from(bits_per_sample)
                                    * u64::from(rate)
                                    * u64::from(channels))
                                    / 1000) as u32
                            });

                        (duration_seconds, sample_rate, channels, codec, bitrate_kbps)
                    });

                Ok(MediaMetadata {
                    format,
                    duration_seconds,
                    codec,
                    sample_rate,
                    channels,
                    bitrate_kbps,
                    tags: HashMap::new(),
                    size_bytes,
                })
            }
            Err(_) => Ok(MediaMetadata {
                format,
                duration_seconds: 0.0,
                codec: None,
                sample_rate: None,
                channels: None,
                bitrate_kbps: None,
                tags: HashMap::new(),
                size_bytes,
            }),
        }
    }
}

#[cfg(not(feature = "transcription"))]
pub fn transcribe(
    _path: &Path,
    _opts: TranscribeOptions,
) -> Result<TranscribeOutput, ProcessingError> {
    Err(ProcessingError::FeatureDisabled(
        "transcription".to_string(),
    ))
}

#[cfg(feature = "transcription")]
pub fn transcribe(
    path: &Path,
    opts: TranscribeOptions,
) -> Result<TranscribeOutput, ProcessingError> {
    if !opts.model_path.exists() {
        return Err(ProcessingError::ModelNotFound(opts.model_path.clone()));
    }

    transcribe_impl(path, &opts)
}

#[cfg(feature = "transcription")]
const TARGET_SAMPLE_RATE: u32 = 16_000;

#[cfg(feature = "transcription")]
fn transcribe_impl(
    path: &Path,
    opts: &TranscribeOptions,
) -> Result<TranscribeOutput, ProcessingError> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    let metadata = media_metadata(path)?;
    let duration_seconds = metadata.duration_seconds;
    let audio_data = decode_to_f32_pcm(path)?;

    let model_path = opts
        .model_path
        .to_str()
        .ok_or_else(|| ProcessingError::ModelNotFound(opts.model_path.clone()))?;

    let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
        .map_err(|_| ProcessingError::ModelNotFound(opts.model_path.clone()))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    if let Some(language) = opts.language.as_deref() {
        params.set_language(Some(language));
    }
    params.set_print_progress(false);
    params.set_print_special(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    let mut state = ctx
        .create_state()
        .map_err(|err| ProcessingError::FormatError(err.to_string()))?;
    state
        .full(params, &audio_data)
        .map_err(|err| ProcessingError::FormatError(err.to_string()))?;

    let num_segments = state
        .full_n_segments()
        .map_err(|err| ProcessingError::FormatError(err.to_string()))?;
    let mut transcript = String::new();
    let mut segments = Vec::with_capacity(num_segments as usize);

    for index in 0..num_segments {
        let text = state
            .full_get_segment_text(index)
            .map_err(|err| ProcessingError::FormatError(err.to_string()))?;
        let start_seconds = state
            .full_get_segment_t0(index)
            .map_err(|err| ProcessingError::FormatError(err.to_string()))?
            as f64
            / 100.0;
        let end_seconds = state
            .full_get_segment_t1(index)
            .map_err(|err| ProcessingError::FormatError(err.to_string()))?
            as f64
            / 100.0;

        transcript.push_str(&text);
        segments.push(Segment {
            start_seconds,
            end_seconds,
            text,
        });
    }

    Ok(TranscribeOutput {
        metadata,
        transcript,
        segments: opts.timestamps.then_some(segments),
        duration_seconds,
    })
}

#[cfg(feature = "transcription")]
fn decode_to_f32_pcm(path: &Path) -> Result<Vec<f32>, ProcessingError> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let src = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(src), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|err| ProcessingError::FormatError(err.to_string()))?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| ProcessingError::CorruptFile("no audio track found".to_string()))?
        .clone();
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|err| ProcessingError::FormatError(err.to_string()))?;

    let mut interleaved = Vec::new();
    let mut source_sample_rate = track.codec_params.sample_rate;
    let mut source_channels = track.codec_params.channels.map(|channels| channels.count());

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => {
                return Err(ProcessingError::FormatError(
                    "decoder reset required while reading audio".to_string(),
                ));
            }
            Err(err) => return Err(ProcessingError::FormatError(err.to_string())),
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                source_sample_rate.get_or_insert(decoded.spec().rate);
                source_channels.get_or_insert(decoded.spec().channels.count());

                let mut sample_buffer =
                    SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
                sample_buffer.copy_interleaved_ref(decoded);
                interleaved.extend_from_slice(sample_buffer.samples());
            }
            Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::IoError(_)) => continue,
            Err(SymphoniaError::ResetRequired) => {
                return Err(ProcessingError::FormatError(
                    "decoder reset required while decoding audio".to_string(),
                ));
            }
            Err(err) => return Err(ProcessingError::FormatError(err.to_string())),
        }
    }

    if interleaved.is_empty() {
        return Err(ProcessingError::CorruptFile(
            "no decodable audio samples found".to_string(),
        ));
    }

    let source_sample_rate = source_sample_rate
        .ok_or_else(|| ProcessingError::FormatError("missing sample rate".to_string()))?;
    let source_channels = source_channels
        .ok_or_else(|| ProcessingError::FormatError("missing channel count".to_string()))?;

    if source_channels == 0 {
        return Err(ProcessingError::FormatError(
            "invalid channel count".to_string(),
        ));
    }

    let mono = downmix_to_mono(&interleaved, source_channels);
    Ok(resample_linear(
        &mono,
        source_sample_rate,
        TARGET_SAMPLE_RATE,
    ))
}

#[cfg(feature = "transcription")]
fn downmix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return interleaved.to_vec();
    }

    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

#[cfg(feature = "transcription")]
fn resample_linear(samples: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if samples.is_empty() || src_rate == dst_rate {
        return samples.to_vec();
    }

    let dst_len =
        ((samples.len() as u64 * u64::from(dst_rate)) / u64::from(src_rate)).max(1) as usize;
    let ratio = src_rate as f64 / dst_rate as f64;
    let mut out = Vec::with_capacity(dst_len);

    for index in 0..dst_len {
        let src_pos = index as f64 * ratio;
        let left = src_pos.floor() as usize;
        let right = (left + 1).min(samples.len() - 1);
        let frac = (src_pos - left as f64) as f32;
        let value = if left == right {
            samples[left]
        } else {
            samples[left] * (1.0 - frac) + samples[right] * frac
        };
        out.push(value);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn feature_disabled_error() {
        #[cfg(not(feature = "transcription"))]
        {
            let opts = TranscribeOptions {
                language: None,
                timestamps: false,
                model_path: PathBuf::from("/fake/model.bin"),
            };
            let result = transcribe(Path::new("/fake/audio.mp3"), opts);
            assert!(matches!(result, Err(ProcessingError::FeatureDisabled(_))));
        }

        #[cfg(feature = "transcription")]
        {
            let _ = true;
        }
    }

    #[test]
    #[cfg(feature = "transcription")]
    fn missing_model_returns_error() {
        let opts = TranscribeOptions {
            language: None,
            timestamps: false,
            model_path: PathBuf::from("/nonexistent/model.bin"),
        };
        let result = transcribe(Path::new("/fake/audio.mp3"), opts);
        assert!(matches!(result, Err(ProcessingError::ModelNotFound(_))));
    }

    #[test]
    fn media_metadata_basic() {
        let mut file = NamedTempFile::with_suffix(".mp3").unwrap();
        file.write_all(b"fake mp3 data").unwrap();

        let meta = media_metadata(file.path()).unwrap();
        assert_eq!(meta.format, "mp3");
        assert!(meta.size_bytes > 0);
    }
}
