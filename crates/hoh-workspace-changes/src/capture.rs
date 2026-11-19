//! Content-addressed whole-file captures around file-tool mutations.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Bytes git probes for NUL before classifying content as binary.
const BINARY_PROBE_BYTES: usize = 8_000;
static TEMPORARY_CAPTURE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Capture {
    Absent,
    Oversized,
    File { file: PathBuf, binary: bool },
}

fn sha1(bytes: &[u8]) -> String {
    let mut padded = bytes.to_vec();
    let bit_len = (padded.len() as u64).wrapping_mul(8);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    let mut h = [
        0x6745_2301_u32,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];
    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 80];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes(chunk[offset..offset + 4].try_into().expect("word"));
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (index, word) in words.into_iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    h.into_iter().map(|word| format!("{word:08x}")).collect()
}

/// Capture at most `max_bytes + 1` bytes of one regular file.
pub async fn capture_file(
    absolute: &Path,
    directory: &Path,
    max_bytes: usize,
) -> std::io::Result<Option<Capture>> {
    let handle = match tokio::fs::File::open(absolute).await {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(Capture::Absent));
        }
        Err(error) => return Err(error),
    };
    if !handle.metadata().await?.is_file() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    handle
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > max_bytes {
        return Ok(Some(Capture::Oversized));
    }
    tokio::fs::create_dir_all(directory).await?;
    let (bytes, digest) = tokio::task::spawn_blocking(move || {
        let digest = sha1(&bytes);
        (bytes, digest)
    })
    .await
    .map_err(std::io::Error::other)?;
    let file = directory.join(digest);
    if !tokio::fs::try_exists(&file).await? {
        let temporary = directory.join(format!(
            ".capture-{}-{}",
            std::process::id(),
            TEMPORARY_CAPTURE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let publish = async {
            let mut output = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .await?;
            output.write_all(&bytes).await?;
            output.flush().await?;
            drop(output);
            match tokio::fs::hard_link(&temporary, &file).await {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error),
            }
        }
        .await;
        let cleanup = tokio::fs::remove_file(&temporary).await;
        publish?;
        if let Err(error) = cleanup
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error);
        }
    }
    Ok(Some(Capture::File {
        file,
        binary: bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0),
    }))
}

/// True only when both captures are known to contain the same bytes.
pub fn same_capture(left: &Capture, right: &Capture) -> bool {
    match (left, right) {
        (Capture::Absent, Capture::Absent) => true,
        (Capture::File { file: left, .. }, Capture::File { file: right, .. }) => left == right,
        _ => false,
    }
}
