//! Concatenated, independently decodable checksummed Zstandard frames.
use crate::{
    Result,
    error::{cause, error},
};
use std::io::{Read, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZstdFrameRange {
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZstdFrameScan {
    pub frames: Vec<ZstdFrameRange>,
    pub torn_start: Option<usize>,
}

pub fn scan_zstd_frames(buffer: &[u8], max_frames: Option<usize>) -> Result<ZstdFrameScan> {
    let mut scan = ZstdFrameScan::default();
    let mut offset = 0;
    macro_rules! need {
        ($length:expr, $start:expr) => {
            if buffer.len() - offset < $length {
                scan.torn_start = Some($start);
                return Ok(scan);
            }
        };
    }
    while offset < buffer.len() {
        let start = offset;
        need!(4, start);
        if buffer[offset..offset + 4] != [0x28, 0xb5, 0x2f, 0xfd] {
            return Err(error(format!(
                "corrupt Zstandard session log: invalid frame magic at byte {offset}"
            )));
        }
        offset += 4;
        need!(1, start);
        let descriptor = buffer[offset];
        offset += 1;
        if descriptor & 0x18 != 0 {
            return Err(error(format!(
                "corrupt Zstandard session log: reserved frame-header bit at byte {}",
                offset - 1
            )));
        }
        let size_flag = descriptor >> 6;
        let single = descriptor & 0x20 != 0;
        let checksum = descriptor & 4 != 0;
        let dict_flag = descriptor & 3;
        let dictionary = if dict_flag == 3 {
            4
        } else {
            usize::from(dict_flag)
        };
        let size_bytes = if size_flag == 0 {
            usize::from(single)
        } else {
            1 << size_flag
        };
        let header = usize::from(!single) + dictionary + size_bytes;
        need!(header, start);
        offset += header;
        loop {
            need!(3, start);
            let block = u32::from(buffer[offset])
                | (u32::from(buffer[offset + 1]) << 8)
                | (u32::from(buffer[offset + 2]) << 16);
            offset += 3;
            let last = block & 1 != 0;
            let kind = (block >> 1) & 3;
            if kind == 3 {
                return Err(error(format!(
                    "corrupt Zstandard session log: reserved block type at byte {}",
                    offset - 3
                )));
            }
            let payload = if kind == 1 { 1 } else { (block >> 3) as usize };
            need!(payload, start);
            offset += payload;
            if last {
                break;
            }
        }
        if checksum {
            need!(4, start);
            offset += 4;
        }
        scan.frames.push(ZstdFrameRange { start, end: offset });
        if max_frames == Some(scan.frames.len()) {
            return Ok(scan);
        }
    }
    Ok(scan)
}

pub(crate) fn compress_sync(input: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ::zstd::stream::Encoder::new(Vec::new(), 0)?;
    encoder.include_checksum(true)?;
    encoder.write_all(input)?;
    Ok(encoder.finish()?)
}
pub async fn compress_zstd_frame(input: impl AsRef<[u8]>) -> Result<Vec<u8>> {
    let bytes = input.as_ref().to_vec();
    tokio::task::spawn_blocking(move || compress_sync(&bytes)).await?
}
pub(crate) fn decompress_sync(input: &[u8]) -> Result<Vec<u8>> {
    Ok(::zstd::stream::decode_all(input)?)
}
pub async fn decompress_zstd_frame(input: &[u8]) -> Result<Vec<u8>> {
    let bytes = input.to_vec();
    tokio::task::spawn_blocking(move || decompress_sync(&bytes)).await?
}
pub async fn decompress_zstd_prefix(input: &[u8]) -> Result<Vec<u8>> {
    let bytes = input.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut decoder = ::zstd::stream::read::Decoder::new(bytes.as_slice())?;
        let mut output = Vec::new();
        let mut scratch = [0; 64 * 1024];
        loop {
            match decoder.read(&mut scratch) {
                Ok(0) => return Ok(output),
                Ok(count) => output.extend_from_slice(&scratch[..count]),
                Err(failure) if failure.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(output);
                }
                Err(failure) => return Err(failure.into()),
            }
        }
    })
    .await?
}

/// Owned output buffers replace Node's borrowed scratch views.
#[derive(Default)]
pub struct ZstdFrameDecoder {
    started: bool,
    closed: bool,
    context: Option<::zstd::stream::raw::Decoder<'static>>,
    output: Vec<u8>,
}
impl ZstdFrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn create() -> Option<Self> {
        Some(Self::default())
    }
    pub fn with_chunk_size(bytes: usize) -> Self {
        Self {
            output: vec![0; bytes.max(1)],
            ..Self::default()
        }
    }
    pub fn decode<'a>(
        &'a mut self,
        source: &'a [u8],
        frames: &'a [ZstdFrameRange],
    ) -> Result<impl Iterator<Item = Result<Vec<u8>>> + 'a> {
        if self.started {
            return Err(error("Zstandard frame decoder was already started"));
        }
        if self.closed {
            return Err(error("cannot start a closed Zstandard frame decoder"));
        }
        self.started = true;
        Ok(FrameIterator {
            decoder: self,
            source,
            frames: frames.iter(),
        })
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.context.take();
    }
}
struct FrameIterator<'a> {
    decoder: &'a mut ZstdFrameDecoder,
    source: &'a [u8],
    frames: std::slice::Iter<'a, ZstdFrameRange>,
}
impl Iterator for FrameIterator<'_> {
    type Item = Result<Vec<u8>>;
    fn next(&mut self) -> Option<Self::Item> {
        let Some(frame) = self.frames.next() else {
            self.decoder.close();
            return None;
        };
        let result = (|| -> Result<Vec<u8>> {
            use ::zstd::stream::raw::Operation;
            if self.decoder.context.is_none() {
                self.decoder.context = Some(::zstd::stream::raw::Decoder::new()?);
            }
            if self.decoder.output.is_empty() {
                self.decoder.output.resize(1024 * 1024, 0);
            }
            let context = self
                .decoder
                .context
                .as_mut()
                .ok_or_else(|| error("cannot decode with a closed Zstandard frame decoder"))?;
            context.reinit()?;
            // Buffer.subarray clamps caller-supplied ranges to the source bounds.
            let start = frame.start.min(self.source.len());
            let end = frame.end.min(self.source.len()).max(start);
            let input = &self.source[start..end];
            let mut offset = 0;
            let mut decoded = vec![];
            loop {
                let status = context.run_on_buffers(&input[offset..], &mut self.decoder.output)?;
                offset += status.bytes_read;
                decoded.extend_from_slice(&self.decoder.output[..status.bytes_written]);
                if status.remaining == 0 {
                    if offset != input.len() {
                        return Err(error("Zstandard frame decoder left trailing input"));
                    }
                    return Ok(decoded);
                }
                if status.bytes_read == 0 && status.bytes_written == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "incomplete Zstandard frame",
                    )
                    .into());
                }
            }
        })()
        .map_err(|e| {
            cause(
                format!(
                    "corrupt Zstandard session log: frame at byte {} failed validation",
                    frame.start
                ),
                e,
            )
        });
        if result.is_err() {
            self.frames = [].iter();
            self.decoder.close();
        }
        Some(result)
    }
}
impl Drop for FrameIterator<'_> {
    fn drop(&mut self) {
        self.decoder.close();
    }
}
pub fn create_zstd_frame_decoder() -> ZstdFrameDecoder {
    ZstdFrameDecoder::default()
}
