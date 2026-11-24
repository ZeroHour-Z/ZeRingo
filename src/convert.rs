//! NCM -> MP3 / WAV。

use crate::ncm::{self, InnerFormat};
use std::io::Cursor;
use std::path::{Path, PathBuf};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Mp3,
    Wav,
}

impl Target {
    fn ext(self) -> &'static str {
        match self {
            Target::Mp3 => "mp3",
            Target::Wav => "wav",
        }
    }
}

/// 解码后的 PCM，交错排列，值按 i32 满幅度存储。
struct Pcm {
    samples: Vec<i32>,
    channels: usize,
    sample_rate: u32,
    bits: u32,
}

fn decode(audio: Vec<u8>, format: InnerFormat) -> Result<Pcm, String> {
    let mut hint = Hint::new();
    hint.with_extension(match format {
        InnerFormat::Mp3 => "mp3",
        InnerFormat::Flac => "flac",
    });
    let mss = MediaSourceStream::new(Box::new(Cursor::new(audio)), Default::default());
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("无法识别音频：{e}"))?;
    let mut reader = probed.format;
    let track = reader.default_track().ok_or("文件中没有音轨")?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let bits = match params.bits_per_sample {
        Some(b @ (24 | 32)) => b.min(24),
        _ => 16,
    };
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| format!("不支持的编码：{e}"))?;

    let mut samples = Vec::new();
    let mut channels = params.channels.map(|c| c.count()).unwrap_or(0);
    let mut sample_rate = params.sample_rate.unwrap_or(0);
    let mut buf: Option<SampleBuffer<i32>> = None;
    loop {
        let packet = match reader.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(format!("读取音频失败：{e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // 个别损坏的帧跳过即可
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("解码失败：{e}")),
        };
        let spec = *decoded.spec();
        channels = spec.channels.count();
        sample_rate = spec.rate;
        let b = buf.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if b.capacity() < decoded.capacity() * channels {
            *b = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        b.copy_interleaved_ref(decoded);
        samples.extend_from_slice(b.samples());
    }
    if channels == 0 || sample_rate == 0 || samples.is_empty() {
        return Err("音频内容为空".into());
    }
    Ok(Pcm { samples, channels, sample_rate, bits })
}

fn write_wav(pcm: &Pcm, out: &Path) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: pcm.channels as u16,
        sample_rate: pcm.sample_rate,
        bits_per_sample: pcm.bits as u16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(out, spec).map_err(|e| e.to_string())?;
    let shift = 32 - pcm.bits;
    for &s in &pcm.samples {
        w.write_sample(s >> shift).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

fn encode_mp3(pcm: &Pcm) -> Result<Vec<u8>, String> {
    use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushNoGap, MonoPcm, Quality};
    let err = |e: &dyn std::fmt::Debug| format!("MP3 编码失败：{e:?}");

    let mut b = Builder::new().ok_or("无法初始化 LAME")?;
    let ch = pcm.channels.min(2);
    b.set_num_channels(ch as u8).map_err(|e| err(&e))?;
    b.set_sample_rate(pcm.sample_rate).map_err(|e| err(&e))?;
    b.set_brate(Bitrate::Kbps320).map_err(|e| err(&e))?;
    b.set_quality(Quality::Best).map_err(|e| err(&e))?;
    let mut enc = b.build().map_err(|e| err(&e))?;

    // 转成 16 位；超过 2 声道时只取前两个
    let frames = pcm.samples.len() / pcm.channels;
    let get = |f: usize, c: usize| (pcm.samples[f * pcm.channels + c] >> 16) as i16;
    let mut out = Vec::new();
    const CHUNK: usize = 1152 * 64;
    let mut left = Vec::with_capacity(CHUNK);
    let mut right = Vec::with_capacity(CHUNK);
    for start in (0..frames).step_by(CHUNK) {
        let end = (start + CHUNK).min(frames);
        left.clear();
        right.clear();
        left.extend((start..end).map(|f| get(f, 0)));
        out.reserve(mp3lame_encoder::max_required_buffer_size(end - start));
        if ch == 2 {
            right.extend((start..end).map(|f| get(f, 1)));
            enc.encode_to_vec(DualPcm { left: &left, right: &right }, &mut out)
        } else {
            enc.encode_to_vec(MonoPcm(&left), &mut out)
        }
        .map_err(|e| err(&e))?;
    }
    out.reserve(7200);
    enc.flush_to_vec::<FlushNoGap>(&mut out).map_err(|e| err(&e))?;
    Ok(out)
}

/// 输出路径：同目录同名，扩展名替换；若已存在则加 " (1)"、" (2)"…
fn output_path(input: &Path, target: Target) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    let dir = input.parent().unwrap_or(Path::new("."));
    let mut p = dir.join(format!("{stem}.{}", target.ext()));
    let mut n = 1;
    while p.exists() {
        p = dir.join(format!("{stem} ({n}).{}", target.ext()));
        n += 1;
    }
    p
}

pub fn convert(input: &Path, target: Target) -> Result<PathBuf, String> {
    let data = std::fs::read(input).map_err(|e| format!("读取失败：{e}"))?;
    let dec = ncm::decrypt(&data)?;
    let out = output_path(input, target);
    match (dec.format, target) {
        // 内部本来就是 MP3，直接写出，无损且最快
        (InnerFormat::Mp3, Target::Mp3) => {
            std::fs::write(&out, &dec.audio).map_err(|e| format!("写入失败：{e}"))?
        }
        (fmt, Target::Mp3) => {
            let mp3 = encode_mp3(&decode(dec.audio, fmt)?)?;
            std::fs::write(&out, mp3).map_err(|e| format!("写入失败：{e}"))?
        }
        (fmt, Target::Wav) => {
            let pcm = decode(dec.audio, fmt)?;
            if let Err(e) = write_wav(&pcm, &out) {
                let _ = std::fs::remove_file(&out);
                return Err(format!("写入失败：{e}"));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, rate: u32) -> Pcm {
        let mut samples = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let v = (i as f64 * 440.0 * std::f64::consts::TAU / rate as f64).sin();
            let s = (v * 0.5 * i32::MAX as f64) as i32;
            samples.push(s);
            samples.push(-s);
        }
        Pcm { samples, channels: 2, sample_rate: rate, bits: 16 }
    }

    #[test]
    fn mp3_inside_ncm_to_wav_and_mp3() {
        let mp3 = encode_mp3(&sine(44_100, 44_100)).unwrap();
        let ncm = ncm::encrypt_for_test(&mp3, b"0123456789abcdefTESTKEY");
        let dir = std::env::temp_dir().join("ncm-converter-test");
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("song.ncm");
        std::fs::write(&input, &ncm).unwrap();

        let out = convert(&input, Target::Mp3).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), mp3);

        let wav = convert(&input, Target::Wav).unwrap();
        let r = hound::WavReader::open(&wav).unwrap();
        assert_eq!(r.spec().channels, 2);
        assert_eq!(r.spec().sample_rate, 44_100);
        assert!(r.duration() > 40_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod flac_tests {
    use super::*;

    /// 需要外部 FLAC 文件：NCM_TEST_FLAC=/path/x.flac cargo test -- --ignored
    #[test]
    #[ignore]
    fn flac_inside_ncm() {
        let flac = std::fs::read(std::env::var("NCM_TEST_FLAC").unwrap()).unwrap();
        let dir = std::env::temp_dir().join("ncm-converter-flac-test");
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("hi-res.ncm");
        std::fs::write(&input, ncm::encrypt_for_test(&flac, b"another-test-key-123")).unwrap();

        let wav = convert(&input, Target::Wav).unwrap();
        let r = hound::WavReader::open(&wav).unwrap();
        println!("wav spec {:?} frames {}", r.spec(), r.duration());
        let mp3 = convert(&input, Target::Mp3).unwrap();
        println!("mp3 {} bytes at {}", std::fs::metadata(&mp3).unwrap().len(), mp3.display());
    }
}
