use anyhow::Result;
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use log::debug;
use std::io::{Seek, Write};
use std::path::Path;

/// Sample rate of every recording Handy captures, saves and uploads.
const RECORDING_SAMPLE_RATE: u32 = 16_000;

/// Read a WAV file and return normalised f32 samples.
pub fn read_wav_samples<P: AsRef<Path>>(file_path: P) -> Result<Vec<f32>> {
    let reader = WavReader::open(file_path.as_ref())?;
    let samples = reader
        .into_samples::<i16>()
        .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
        .collect::<Result<Vec<f32>, _>>()?;
    Ok(samples)
}

/// Verify a WAV file by reading it back and checking the sample count.
pub fn verify_wav_file<P: AsRef<Path>>(file_path: P, expected_samples: usize) -> Result<()> {
    let reader = WavReader::open(file_path.as_ref())?;
    let actual_samples = reader.len() as usize;
    if actual_samples != expected_samples {
        anyhow::bail!(
            "WAV sample count mismatch: expected {}, got {}",
            expected_samples,
            actual_samples
        );
    }
    Ok(())
}

/// The one encoding used for both recordings on disk and audio uploaded for
/// cloud transcription: 16 kHz mono 16-bit PCM, matching the recorder.
fn recording_wav_spec() -> WavSpec {
    WavSpec {
        channels: 1,
        sample_rate: RECORDING_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    }
}

/// Convert one normalized f32 sample to the 16-bit PCM value written to disk and
/// uploaded. The `as` cast saturates out-of-range and non-finite samples, so an
/// unexpected value can never produce an invalid sample.
fn to_pcm16(sample: f32) -> i16 {
    (sample * i16::MAX as f32) as i16
}

/// Write `samples` as a 16 kHz mono 16-bit PCM WAV stream. Shared by
/// [`save_wav_file`] (files) and [`encode_wav`] (in-memory uploads) so both
/// always agree on format and conversion.
fn write_wav<W: Write + Seek>(writer: W, samples: &[f32]) -> Result<()> {
    let mut writer = WavWriter::new(writer, recording_wav_spec())?;
    for sample in samples {
        writer.write_sample(to_pcm16(*sample))?;
    }
    writer.finalize()?;
    Ok(())
}

/// Save audio samples as a WAV file
pub fn save_wav_file<P: AsRef<Path>>(file_path: P, samples: &[f32]) -> Result<()> {
    let file = std::fs::File::create(file_path.as_ref())?;
    write_wav(std::io::BufWriter::new(file), samples)?;
    debug!("Saved WAV file: {:?}", file_path.as_ref());
    Ok(())
}

/// Encode audio samples as an in-memory 16 kHz mono 16-bit PCM WAV file.
///
/// Used for cloud transcription, where the audio is uploaded directly instead of
/// being written to a temporary file.
pub fn encode_wav(samples: &[f32]) -> Result<Vec<u8>> {
    // 16-bit mono samples plus the canonical 44-byte PCM header.
    let mut buffer = std::io::Cursor::new(Vec::with_capacity(samples.len() * 2 + 44));
    write_wav(&mut buffer, samples)?;
    Ok(buffer.into_inner())
}
