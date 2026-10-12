use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{CodecType, Decoder, DecoderOptions, CODEC_TYPE_AAC, CODEC_TYPE_MP3},
    errors::Error,
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
    units::{Time, TimeBase},
};

pub struct Block {
    pub samples: Vec<f32>,
    pub channels: usize,
    pub rate: u32,
    pub start_seconds: f64,
}

pub struct AudioDecoder {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    time_base: TimeBase,
    discard_before: f64,
    pub duration: Option<f64>,
    cancel: Option<tokio_util::sync::CancellationToken>,
}

impl AudioDecoder {
    pub fn open(path: &Path) -> Result<Self, String> {
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "mp3" | "aac" | "m4a" | "mp4") {
            return Err("Choose a local MP3, AAC, or M4A file.".into());
        }
        let file = File::open(path).map_err(|e| format!("Cannot open audio file: {e}"))?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Audio source must be a regular file.".into());
        }
        Self::from_source(Box::new(file), &extension, None)
    }

    pub fn from_source(
        mut source: Box<dyn MediaSource>,
        extension: &str,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<Self, String> {
        // Symphonia's seekable MP4 initialization scans every fragment and reads
        // 64 KiB of each skipped mdat. Use its progressive initialization for
        // fast-start HTTP M4A, then enable random access for playback/seeking.
        let seekable = Arc::new(AtomicBool::new(false));
        let progressive =
            cancel.is_some() && extension == "m4a" && has_fast_start(source.as_mut())?;
        let source = if progressive {
            Box::new(ProgressiveSource {
                inner: source,
                seekable: seekable.clone(),
            }) as Box<dyn MediaSource>
        } else {
            source
        };
        let source = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        hint.with_extension(extension);
        let reader = symphonia::default::get_probe()
            .format(
                &hint,
                source,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(|e| format!("Unsupported or damaged audio file: {e}"))?
            .format;
        seekable.store(true, Ordering::Release);
        let track = reader
            .tracks()
            .iter()
            .find(|t| supported(t.codec_params.codec))
            .ok_or("The file contains no supported MP3 or AAC-LC audio track.")?;
        let time_base = track
            .codec_params
            .time_base
            .ok_or("Audio track has no time base.")?;
        let duration = track.codec_params.n_frames.map(|n| seconds(time_base, n));
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| format!("Cannot create audio decoder: {e}"))?;
        let track = track.id;
        Ok(Self {
            reader,
            decoder,
            track,
            time_base,
            discard_before: 0.0,
            duration,
            cancel,
        })
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    }

    pub fn seek(&mut self, target: f64) -> Result<(), String> {
        // Preroll restores MP3's bit reservoir and AAC overlap history.
        self.reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from((target - 0.15).max(0.0)),
                    track_id: Some(self.track),
                },
            )
            .map_err(|e| format!("Cannot seek in this file: {e}"))?;
        self.decoder.reset();
        self.discard_before = target;
        Ok(())
    }

    pub fn next_block(&mut self) -> Result<Option<Block>, String> {
        for _ in 0..256 {
            let packet = match self.reader.next_packet() {
                Ok(packet) => packet,
                Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(None)
                }
                Err(e) => return Err(format!("Cannot read audio packet: {e}")),
            };
            if packet.track_id() != self.track {
                continue;
            }
            let audio = match self.decoder.decode(&packet) {
                Ok(audio) => audio,
                Err(Error::DecodeError(_)) => continue,
                Err(e) => return Err(format!("Cannot decode audio packet: {e}")),
            };
            let channels = audio.spec().channels.count();
            let rate = audio.spec().rate;
            if !(1..=2).contains(&channels) || rate == 0 || audio.capacity() > 65536 {
                return Err(
                    "Only mono and stereo audio with bounded packet sizes are supported.".into(),
                );
            }
            let mut samples = SampleBuffer::<f32>::new(audio.capacity() as u64, *audio.spec());
            samples.copy_interleaved_ref(audio);
            let start = seconds(self.time_base, packet.ts());
            let skip = skip_frames(self.discard_before, start, rate).min(samples.len() / channels);
            if skip == samples.len() / channels {
                continue;
            }
            return Ok(Some(Block {
                samples: samples.samples()[skip * channels..].to_vec(),
                channels,
                rate,
                start_seconds: start + skip as f64 / rate as f64,
            }));
        }
        Err("Too many invalid or empty audio packets.".into())
    }
}

fn supported(codec: CodecType) -> bool {
    matches!(codec, CODEC_TYPE_MP3 | CODEC_TYPE_AAC)
}
fn seconds(base: TimeBase, ts: u64) -> f64 {
    let t = base.calc_time(ts);
    t.seconds as f64 + t.frac
}
fn skip_frames(target: f64, start: f64, rate: u32) -> usize {
    (((target - start).max(0.0) * rate as f64 - 1e-7)
        .ceil()
        .max(0.0)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trims_seek_inside_packet_at_sample_precision() {
        assert_eq!(skip_frames(2.0 + 73.0 / 44100.0, 2.0, 44100), 73);
        assert_eq!(skip_frames(1.5, 2.0, 48000), 0);
        assert_eq!(skip_frames(2.00001, 2.0, 48000), 1);
    }
}

/// A fast-start HTTP MP4 is opened progressively; subsequent seeks use ranges.
struct ProgressiveSource {
    inner: Box<dyn MediaSource>,
    seekable: Arc<AtomicBool>,
}
impl Read for ProgressiveSource {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.inner.read(output)
    }
}
impl Seek for ProgressiveSource {
    fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        self.inner.seek(offset)
    }
}
impl MediaSource for ProgressiveSource {
    fn is_seekable(&self) -> bool {
        self.seekable.load(Ordering::Acquire) && self.inner.is_seekable()
    }
    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }
}

/// Inspect only top-level headers. Tail-moov MP4 retains normal seekable probing;
/// fast-start/fragmented YouTube M4A avoids scanning every media fragment.
fn has_fast_start(source: &mut dyn MediaSource) -> Result<bool, String> {
    let result = (|| -> io::Result<bool> {
        let length = source
            .byte_len()
            .ok_or_else(|| io::Error::other("HTTP audio has no length."))?;
        source.seek(SeekFrom::Start(0))?;
        for _ in 0..64 {
            let start = source.stream_position()?;
            let mut header = [0; 8];
            source.read_exact(&mut header)?;
            let mut size = u32::from_be_bytes(header[..4].try_into().unwrap()) as u64;
            let mut header_len = 8;
            if size == 1 {
                let mut extended = [0; 8];
                source.read_exact(&mut extended)?;
                size = u64::from_be_bytes(extended);
                header_len = 16;
            }
            if size == 0 {
                size = length - start;
            }
            let end = start
                .checked_add(size)
                .ok_or_else(|| io::Error::other("Invalid MP4 atom length."))?;
            if size < header_len || end > length {
                return Err(io::Error::other("Invalid MP4 atom length."));
            }
            match &header[4..] {
                b"moov" => return Ok(true),
                b"mdat" => return Ok(false),
                _ => {}
            }
            source.seek(SeekFrom::Start(end))?;
        }
        Err(io::Error::other(
            "MP4 initialization exceeds the atom limit.",
        ))
    })();
    source
        .seek(SeekFrom::Start(0))
        .map_err(|_| "Cannot rewind HTTP audio.".to_owned())?;
    result.map_err(|e| format!("Cannot inspect HTTP MP4: {e}"))
}
