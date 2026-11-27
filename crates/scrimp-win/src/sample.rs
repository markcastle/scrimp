//! Sampling: compressing a few pieces of a file in memory, to see whether
//! the whole file would shrink, without changing it.
//!
//! Automatic mode samples large files of a kind it doesn't know, because
//! many are compressed already (game archives, disk images, encrypted
//! containers) and finding that out by compressing all of a 20 GB file
//! takes minutes. Three 64 KB pieces, from the start, the middle and the
//! end, take milliseconds.
//!
//! The pieces are compressed with Windows' own Compression API
//! (`cabinet.dll`), so Scrimp still adds no compression engine of its own.
//! It uses XPRESS with Huffman coding in 8 KB blocks, which is how WOF's
//! XPRESS8K, the method automatic mode picks for these files, works.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use windows::Win32::Storage::Compression::{
    COMPRESS_ALGORITHM_XPRESS_HUFF, COMPRESSOR_HANDLE, CloseCompressor, Compress, CreateCompressor,
};

use crate::error::Error;

/// The size of each piece read from the file.
pub const PIECE: u64 = 64 * 1024;
/// WOF's XPRESS8K compresses in blocks of this size, each on its own.
const BLOCK: usize = 8 * 1024;

/// Bytes read from a file and what they compressed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub read: u64,
    pub compressed: u64,
}

/// Where the pieces start in a file of `len` bytes: the start, the middle
/// and the last piece, or fewer for a file too short for three.
fn offsets(len: u64) -> Vec<u64> {
    if len <= PIECE {
        return vec![0];
    }
    let mut out = vec![0, (len / 2).saturating_sub(PIECE / 2), len - PIECE];
    out.dedup();
    out
}

/// Windows' compressor, closed when dropped.
struct Compressor(COMPRESSOR_HANDLE);

impl Compressor {
    fn new() -> Result<Compressor, Error> {
        let mut handle = COMPRESSOR_HANDLE::default();
        // SAFETY: `handle` is a live local that receives the new compressor.
        unsafe { CreateCompressor(COMPRESS_ALGORITHM_XPRESS_HUFF, None, &mut handle) }
            .map_err(|e| Error::from_windows(&e, crate::error::Call::Other))?;
        Ok(Compressor(handle))
    }

    /// The size `data` compresses to. Data that would grow counts as its
    /// own size, as WOF would store that block uncompressed.
    fn compressed_size(&self, data: &[u8], scratch: &mut [u8]) -> u64 {
        let mut size = 0usize;
        // SAFETY: `data` and `scratch` are live slices whose lengths are
        // passed with them; `size` is a live local.
        let done = unsafe {
            Compress(
                self.0,
                Some(data.as_ptr().cast()),
                data.len(),
                Some(scratch.as_mut_ptr().cast()),
                scratch.len(),
                &mut size,
            )
        };
        match done {
            Ok(()) => size.min(data.len()) as u64,
            // Too small a buffer means it didn't shrink.
            Err(_) => data.len() as u64,
        }
    }
}

impl Drop for Compressor {
    fn drop(&mut self) {
        // SAFETY: the compressor was created by CreateCompressor and is
        // closed once, here.
        unsafe {
            let _ = CloseCompressor(self.0);
        }
    }
}

/// Reads up to three pieces of the file at `path` and compresses them in
/// memory. Opens the file for reading only, which never undoes WOF
/// compression.
pub fn sample(path: &Path) -> Result<Sample, Error> {
    let mut file = std::fs::File::open(path).map_err(|e| Error::from_io(&e))?;
    let len = file.metadata().map_err(|e| Error::from_io(&e))?.len();
    let compressor = Compressor::new()?;
    let mut piece = vec![0u8; PIECE as usize];
    // Room for a block that grows, which XPRESS allows for.
    let mut scratch = vec![0u8; BLOCK * 2 + 1024];
    let mut result = Sample { read: 0, compressed: 0 };
    for at in offsets(len) {
        file.seek(SeekFrom::Start(at)).map_err(|e| Error::from_io(&e))?;
        let mut filled = 0;
        while filled < piece.len() {
            match file.read(&mut piece[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::from_io(&e)),
            }
        }
        for block in piece[..filled].chunks(BLOCK) {
            result.read += block.len() as u64;
            result.compressed += compressor.compressed_size(block, &mut scratch);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_come_from_the_start_middle_and_end() {
        assert_eq!(offsets(0), [0]);
        assert_eq!(offsets(PIECE), [0]);
        assert_eq!(offsets(PIECE + 1), [0, 1]);
        let mb = 1024 * 1024;
        assert_eq!(offsets(4 * mb), [0, 2 * mb - PIECE / 2, 4 * mb - PIECE]);
    }

    fn file_of(dir: &Path, name: &str, data: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, data).unwrap();
        p
    }

    /// Bytes that look random, as compressed or encrypted data does.
    fn noise(len: usize) -> Vec<u8> {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn text_shrinks_and_noise_doesnt() {
        let dir = tempfile::tempdir().unwrap();
        let mb = 1024 * 1024;
        let text = "Row 1: Example.dat moved to D:\\Archive on day 12.\n"
            .repeat(4 * mb / 50)
            .into_bytes();
        let s = sample(&file_of(dir.path(), "levels.bin", &text)).unwrap();
        assert_eq!(s.read, 3 * PIECE);
        assert!(s.compressed * 2 < s.read, "text roughly halves at least: {s:?}");

        let s = sample(&file_of(dir.path(), "Example.pak", &noise(4 * mb))).unwrap();
        assert_eq!(s.read, 3 * PIECE);
        assert!(s.compressed * 100 > s.read * 95, "noise doesn't shrink: {s:?}");

        let s = sample(&file_of(dir.path(), "tiny.bin", b"short")).unwrap();
        assert_eq!(s.read, 5);
    }

    #[test]
    fn sampling_changes_nothing_and_leaves_wof_files_compressed() {
        let dir = tempfile::tempdir().unwrap();
        let text = "Row 1: Example.dat moved to D:\\Archive.\n".repeat(60_000).into_bytes();
        let p = file_of(dir.path(), "levels.bin", &text);
        crate::compress(&p, scrimp_core::Algorithm::Xpress8k).unwrap();
        let before = crate::inspect(&p).unwrap();
        sample(&p).unwrap();
        let after = crate::inspect(&p).unwrap();
        assert_eq!(after.state, before.state);
        assert_eq!(after.on_disk, before.on_disk);
        assert_eq!(std::fs::read(&p).unwrap(), text);
    }

    #[test]
    fn a_missing_file_is_an_error() {
        assert!(sample(Path::new(r"C:\Scrimp test that doesn't exist\nothing.bin")).is_err());
    }
}
