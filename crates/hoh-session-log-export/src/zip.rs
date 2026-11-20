//! Small deterministic ZIP writer using only dependencies already locked by the workspace.

use flate2::{Compression, write::DeflateEncoder};
use futures_util::{Stream, StreamExt};
use hoh_error::BoxError;
use hoh_timeout::AbortSignal;
use std::io::{self, Write};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    pub path: String,
    pub data: Vec<u8>,
}

impl ZipEntry {
    pub fn bytes(path: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            data: data.into(),
        }
    }

    pub fn text(path: impl Into<String>, text: impl Into<String>) -> Self {
        Self::bytes(path, text.into().into_bytes())
    }
}

struct CentralEntry {
    name: Vec<u8>,
    crc: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    offset: u32,
}

struct CountingWriter<W> {
    inner: W,
    written: u64,
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(bytes)?;
        self.written = self
            .written
            .checked_add(count as u64)
            .ok_or_else(|| too_large("ZIP archive"))?;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Incremental classic-ZIP writer. Entry sizes are recorded in data descriptors,
/// so callers may feed a file stream without retaining the whole object.
pub(crate) struct StreamingZipWriter<W> {
    output: CountingWriter<W>,
    level: u8,
    central: Vec<CentralEntry>,
}

impl<W: Write> StreamingZipWriter<W> {
    pub(crate) fn new(output: W, level: u8) -> io::Result<Self> {
        if level > 9 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ZIP compression level must be in 0..=9",
            ));
        }
        Ok(Self {
            output: CountingWriter {
                inner: output,
                written: 0,
            },
            level,
            central: Vec::new(),
        })
    }

    pub(crate) async fn add_bytes(
        &mut self,
        path: &str,
        data: &[u8],
        signal: Option<&AbortSignal>,
    ) -> Result<(), BoxError> {
        tokio::task::yield_now().await;
        throw_if_aborted(signal)?;
        self.start_entry(path).map_err(boxed_io)?;
        let compressed_start = self.output.written;
        let mut encoder =
            DeflateEncoder::new(&mut self.output, Compression::new(u32::from(self.level)));
        throw_if_aborted(signal)?;
        for chunk in data.chunks(1 << 16) {
            tokio::task::yield_now().await;
            throw_if_aborted(signal)?;
            encoder.write_all(chunk).map_err(boxed_io)?;
        }
        encoder.finish().map_err(boxed_io)?;
        throw_if_aborted(signal)?;
        let compressed_size = self.output.written - compressed_start;
        self.complete_entry(
            path,
            crc32fast::hash(data),
            compressed_size,
            data.len() as u64,
        )
        .map_err(boxed_io)
    }

    pub(crate) async fn add_stream<S>(
        &mut self,
        path: &str,
        first: Option<Vec<u8>>,
        stream: &mut S,
        signal: Option<&AbortSignal>,
    ) -> Result<(), BoxError>
    where
        S: Stream<Item = Result<Vec<u8>, BoxError>> + Unpin + ?Sized,
    {
        tokio::task::yield_now().await;
        throw_if_aborted(signal)?;
        self.start_entry(path).map_err(boxed_io)?;
        let compressed_start = self.output.written;
        let mut crc = crc32fast::Hasher::new();
        let mut uncompressed_size = 0_u64;
        {
            let mut encoder =
                DeflateEncoder::new(&mut self.output, Compression::new(u32::from(self.level)));
            throw_if_aborted(signal)?;
            if let Some(chunk) = first {
                tokio::task::yield_now().await;
                throw_if_aborted(signal)?;
                uncompressed_size = feed_chunk(&mut encoder, &mut crc, uncompressed_size, &chunk)?;
            }
            while let Some(chunk) = stream.next().await {
                tokio::task::yield_now().await;
                throw_if_aborted(signal)?;
                let chunk = chunk?;
                uncompressed_size = feed_chunk(&mut encoder, &mut crc, uncompressed_size, &chunk)?;
            }
            throw_if_aborted(signal)?;
            encoder.finish().map_err(boxed_io)?;
        }
        let compressed_size = self.output.written - compressed_start;
        self.complete_entry(path, crc.finalize(), compressed_size, uncompressed_size)
            .map_err(boxed_io)
    }

    pub(crate) fn finish(mut self) -> io::Result<W> {
        let central_offset =
            u32::try_from(self.output.written).map_err(|_| too_large("ZIP archive"))?;
        for entry in &self.central {
            let name_length =
                u16::try_from(entry.name.len()).map_err(|_| too_large("ZIP entry name"))?;
            write_u32(&mut self.output, 0x0201_4b50)?;
            write_u16(&mut self.output, 20)?;
            write_u16(&mut self.output, 20)?;
            write_u16(&mut self.output, 0x0808)?;
            write_u16(&mut self.output, 8)?;
            write_u16(&mut self.output, 0)?;
            write_u16(&mut self.output, 0)?;
            write_u32(&mut self.output, entry.crc)?;
            write_u32(&mut self.output, entry.compressed_size)?;
            write_u32(&mut self.output, entry.uncompressed_size)?;
            write_u16(&mut self.output, name_length)?;
            write_u16(&mut self.output, 0)?;
            write_u16(&mut self.output, 0)?;
            write_u16(&mut self.output, 0)?;
            write_u16(&mut self.output, 0)?;
            write_u32(&mut self.output, 0)?;
            write_u32(&mut self.output, entry.offset)?;
            self.output.write_all(&entry.name)?;
        }
        let count = u16::try_from(self.central.len()).map_err(|_| too_large("ZIP entry count"))?;
        let central_size = u32::try_from(self.output.written - u64::from(central_offset))
            .map_err(|_| too_large("ZIP directory"))?;
        write_u32(&mut self.output, 0x0605_4b50)?;
        write_u16(&mut self.output, 0)?;
        write_u16(&mut self.output, 0)?;
        write_u16(&mut self.output, count)?;
        write_u16(&mut self.output, count)?;
        write_u32(&mut self.output, central_size)?;
        write_u32(&mut self.output, central_offset)?;
        write_u16(&mut self.output, 0)?;
        Ok(self.output.inner)
    }

    fn start_entry(&mut self, path: &str) -> io::Result<()> {
        let name = path.as_bytes();
        let name_length = u16::try_from(name.len()).map_err(|_| too_large("ZIP entry name"))?;
        let offset = u32::try_from(self.output.written).map_err(|_| too_large("ZIP archive"))?;
        write_u32(&mut self.output, 0x0403_4b50)?;
        write_u16(&mut self.output, 20)?;
        write_u16(&mut self.output, 0x0808)?;
        write_u16(&mut self.output, 8)?;
        write_u16(&mut self.output, 0)?;
        write_u16(&mut self.output, 0)?;
        write_u32(&mut self.output, 0)?;
        write_u32(&mut self.output, 0)?;
        write_u32(&mut self.output, 0)?;
        write_u16(&mut self.output, name_length)?;
        write_u16(&mut self.output, 0)?;
        self.output.write_all(name)?;
        self.central.push(CentralEntry {
            name: name.to_vec(),
            crc: 0,
            compressed_size: 0,
            uncompressed_size: 0,
            offset,
        });
        Ok(())
    }

    fn complete_entry(
        &mut self,
        path: &str,
        crc: u32,
        compressed_size: u64,
        uncompressed_size: u64,
    ) -> io::Result<()> {
        let compressed_size =
            u32::try_from(compressed_size).map_err(|_| too_large("compressed ZIP entry"))?;
        let uncompressed_size =
            u32::try_from(uncompressed_size).map_err(|_| too_large("ZIP entry"))?;
        write_u32(&mut self.output, 0x0807_4b50)?;
        write_u32(&mut self.output, crc)?;
        write_u32(&mut self.output, compressed_size)?;
        write_u32(&mut self.output, uncompressed_size)?;
        let entry = self
            .central
            .last_mut()
            .ok_or_else(|| io::Error::other("ZIP entry state missing"))?;
        debug_assert_eq!(entry.name, path.as_bytes());
        entry.crc = crc;
        entry.compressed_size = compressed_size;
        entry.uncompressed_size = uncompressed_size;
        Ok(())
    }
}

fn feed_chunk(
    encoder: &mut impl Write,
    crc: &mut crc32fast::Hasher,
    size: u64,
    chunk: &[u8],
) -> Result<u64, BoxError> {
    if chunk.is_empty() {
        return Ok(size);
    }
    crc.update(chunk);
    encoder.write_all(chunk).map_err(boxed_io)?;
    size.checked_add(chunk.len() as u64)
        .ok_or_else(|| boxed_io(too_large("ZIP entry")))
}

fn boxed_io(error: io::Error) -> BoxError {
    Box::new(error)
}

fn throw_if_aborted(signal: Option<&AbortSignal>) -> Result<(), BoxError> {
    if let Some(signal) = signal {
        signal
            .throw_if_aborted()
            .map_err(|error| Box::new(error) as BoxError)?;
    }
    Ok(())
}

/// Write a portable UTF-8 ZIP archive in input order.
pub fn write_zip(
    output: &mut impl Write,
    entries: impl IntoIterator<Item = ZipEntry>,
    compression_level: u8,
) -> io::Result<()> {
    if compression_level > 9 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ZIP compression level must be in 0..=9",
        ));
    }
    let mut central = Vec::new();
    let mut offset = 0_u64;
    for entry in entries {
        let name = entry.path.into_bytes();
        let name_length = u16::try_from(name.len()).map_err(|_| too_large("ZIP entry name"))?;
        let uncompressed_size =
            u32::try_from(entry.data.len()).map_err(|_| too_large("ZIP entry"))?;
        let crc = crc32fast::hash(&entry.data);
        let mut encoder =
            DeflateEncoder::new(Vec::new(), Compression::new(u32::from(compression_level)));
        encoder.write_all(&entry.data)?;
        let compressed = encoder.finish()?;
        let compressed_size =
            u32::try_from(compressed.len()).map_err(|_| too_large("compressed ZIP entry"))?;
        let local_offset = u32::try_from(offset).map_err(|_| too_large("ZIP archive"))?;

        write_u32(output, 0x0403_4b50)?;
        write_u16(output, 20)?;
        write_u16(output, 0x0800)?;
        write_u16(output, 8)?;
        write_u16(output, 0)?;
        write_u16(output, 0)?;
        write_u32(output, crc)?;
        write_u32(output, compressed_size)?;
        write_u32(output, uncompressed_size)?;
        write_u16(output, name_length)?;
        write_u16(output, 0)?;
        output.write_all(&name)?;
        output.write_all(&compressed)?;
        offset = offset
            .checked_add(30 + u64::from(name_length) + u64::from(compressed_size))
            .ok_or_else(|| too_large("ZIP archive"))?;
        central.push(CentralEntry {
            name,
            crc,
            compressed_size,
            uncompressed_size,
            offset: local_offset,
        });
    }

    let central_offset = u32::try_from(offset).map_err(|_| too_large("ZIP archive"))?;
    for entry in &central {
        let name_length =
            u16::try_from(entry.name.len()).map_err(|_| too_large("ZIP entry name"))?;
        write_u32(output, 0x0201_4b50)?;
        write_u16(output, 20)?;
        write_u16(output, 20)?;
        write_u16(output, 0x0800)?;
        write_u16(output, 8)?;
        write_u16(output, 0)?;
        write_u16(output, 0)?;
        write_u32(output, entry.crc)?;
        write_u32(output, entry.compressed_size)?;
        write_u32(output, entry.uncompressed_size)?;
        write_u16(output, name_length)?;
        write_u16(output, 0)?;
        write_u16(output, 0)?;
        write_u16(output, 0)?;
        write_u16(output, 0)?;
        write_u32(output, 0)?;
        write_u32(output, entry.offset)?;
        output.write_all(&entry.name)?;
        offset = offset
            .checked_add(46 + u64::from(name_length))
            .ok_or_else(|| too_large("ZIP archive"))?;
    }
    let count = u16::try_from(central.len()).map_err(|_| too_large("ZIP entry count"))?;
    let central_size = u32::try_from(offset - u64::from(central_offset))
        .map_err(|_| too_large("ZIP directory"))?;
    write_u32(output, 0x0605_4b50)?;
    write_u16(output, 0)?;
    write_u16(output, 0)?;
    write_u16(output, count)?;
    write_u16(output, count)?;
    write_u32(output, central_size)?;
    write_u32(output, central_offset)?;
    write_u16(output, 0)
}

fn too_large(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{what} exceeds classic ZIP limits"),
    )
}

fn write_u16(output: &mut impl Write, value: u16) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}

fn write_u32(output: &mut impl Write, value: u32) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::StreamingZipWriter;
    use futures_util::stream;
    use std::{
        io::{self, Write},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    struct YieldGatedWriter {
        ready: Arc<AtomicBool>,
        bytes: Vec<u8>,
    }

    impl Write for YieldGatedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.ready.load(Ordering::SeqCst) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "writer was entered before a sibling task could run",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn byte_entries_yield_before_entering_a_synchronous_sink() {
        let ready = Arc::new(AtomicBool::new(false));
        let sibling_ready = ready.clone();
        tokio::spawn(async move {
            sibling_ready.store(true, Ordering::SeqCst);
        });
        let output = YieldGatedWriter {
            ready,
            bytes: Vec::new(),
        };
        let mut archive = StreamingZipWriter::new(output, 6).unwrap();
        archive.add_bytes("entry", b"payload", None).await.unwrap();
        assert!(!archive.finish().unwrap().bytes.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn streamed_entries_yield_before_entering_a_synchronous_sink() {
        let ready = Arc::new(AtomicBool::new(false));
        let sibling_ready = ready.clone();
        tokio::spawn(async move {
            sibling_ready.store(true, Ordering::SeqCst);
        });
        let output = YieldGatedWriter {
            ready,
            bytes: Vec::new(),
        };
        let mut archive = StreamingZipWriter::new(output, 6).unwrap();
        let mut chunks = stream::iter([Ok(b"payload".to_vec())]);
        archive
            .add_stream("entry", None, &mut chunks, None)
            .await
            .unwrap();
        assert!(!archive.finish().unwrap().bytes.is_empty());
    }
}
