//! Minimal, fully deterministic PKZIP (ZIP/OPC) writer.
//!
//! This is a from-scratch implementation of just enough of the ZIP format to
//! produce a valid `.pptx` (OPC) container: STORE (uncompressed) entries only,
//! a fixed DOS timestamp on every entry, and entries written in the exact
//! order `add()` is called. No external `zip`/`flate2` crate is used — see
//! the crate-level determinism note in `lib.rs` for why (a deflate
//! implementation's output can vary across zlib versions, which would break
//! the "same input → byte-identical output" contract).
//!
//! ## Determinism
//! - Compression method is always STORE (0): `compressed_size == uncompressed_size == data.len()`.
//! - Every entry's DOS mtime/mdate is the fixed constant `1980-01-01T00:00:00`
//!   (`mdate = 0x0021`, `mtime = 0x0000`) — never the wall-clock time.
//! - Entries are written in call order (`Vec`, not `HashMap`), so byte layout
//!   only depends on the sequence of `add()` calls made by the caller.

/// Fixed DOS date for 1980-01-01 (the DOS epoch): see the ZIP spec's
/// `dos_date` encoding `((year-1980)<<9) | (month<<5) | day`. For 1980-01-01
/// that's `(0<<9) | (1<<5) | 1 = 0x0021`.
const FIXED_DOS_DATE: u16 = 0x0021;
/// Fixed DOS time (00:00:00).
const FIXED_DOS_TIME: u16 = 0x0000;

const LOCAL_FILE_HEADER_SIG: u32 = 0x0403_4b50;
const CENTRAL_DIR_HEADER_SIG: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIR_SIG: u32 = 0x0605_4b50;

/// Version needed to extract / version made by: 2.0 (STORE + basic ZIP).
const VERSION: u16 = 20;

// ---------------------------------------------------------------------------
// CRC-32 (ISO 3309 / ITU-T V.42), standard polynomial 0xEDB88320.
// ---------------------------------------------------------------------------

/// Build the 256-entry CRC-32 lookup table at compile time (const fn).
const fn build_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const CRC32_TABLE: [u32; 256] = build_crc32_table();

/// Compute the CRC-32 checksum of `data` (standard zlib/PKZIP polynomial).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        let idx = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[idx];
    }
    crc ^ 0xFFFF_FFFF
}

// ---------------------------------------------------------------------------
// Little-endian byte-packing helpers
// ---------------------------------------------------------------------------

fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

// ---------------------------------------------------------------------------
// ZipWriter
// ---------------------------------------------------------------------------

/// A central-directory record, captured when each entry is written, used to
/// emit the central directory at `finish()`.
struct CentralEntry {
    name: String,
    crc32: u32,
    size: u32,
    local_header_offset: u32,
}

/// A minimal STORE-only, deterministic ZIP writer.
///
/// Usage: call [`ZipWriter::add`] for each entry in the exact order they
/// should appear in the archive, then [`ZipWriter::finish`] to obtain the
/// complete archive bytes (local entries + central directory + EOCD).
pub struct ZipWriter {
    buf: Vec<u8>,
    entries: Vec<CentralEntry>,
}

impl ZipWriter {
    pub fn new() -> Self {
        ZipWriter {
            buf: Vec::new(),
            entries: Vec::new(),
        }
    }

    /// Add one STORE (uncompressed) entry named `name` with contents `data`.
    /// `name` must be an ASCII path using `/` separators (e.g. `"ppt/slides/slide1.xml"`).
    pub fn add(&mut self, name: &str, data: &[u8]) {
        let local_header_offset = self.buf.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        let name_bytes = name.as_bytes();

        // --- Local file header ---
        push_u32(&mut self.buf, LOCAL_FILE_HEADER_SIG);
        push_u16(&mut self.buf, VERSION); // version needed to extract
        push_u16(&mut self.buf, 0); // general purpose bit flag
        push_u16(&mut self.buf, 0); // compression method: STORE
        push_u16(&mut self.buf, FIXED_DOS_TIME);
        push_u16(&mut self.buf, FIXED_DOS_DATE);
        push_u32(&mut self.buf, crc);
        push_u32(&mut self.buf, size); // compressed size == uncompressed size (STORE)
        push_u32(&mut self.buf, size);
        push_u16(&mut self.buf, name_bytes.len() as u16);
        push_u16(&mut self.buf, 0); // extra field length
        self.buf.extend_from_slice(name_bytes);
        // no extra field
        self.buf.extend_from_slice(data);

        self.entries.push(CentralEntry {
            name: name.to_string(),
            crc32: crc,
            size,
            local_header_offset,
        });
    }

    /// Finish the archive: append the central directory and the End Of
    /// Central Directory record, and return the complete archive bytes.
    pub fn finish(self) -> Vec<u8> {
        let mut buf = self.buf;
        let cd_start = buf.len() as u32;

        for entry in &self.entries {
            let name_bytes = entry.name.as_bytes();
            push_u32(&mut buf, CENTRAL_DIR_HEADER_SIG);
            push_u16(&mut buf, VERSION); // version made by
            push_u16(&mut buf, VERSION); // version needed to extract
            push_u16(&mut buf, 0); // general purpose bit flag
            push_u16(&mut buf, 0); // compression method: STORE
            push_u16(&mut buf, FIXED_DOS_TIME);
            push_u16(&mut buf, FIXED_DOS_DATE);
            push_u32(&mut buf, entry.crc32);
            push_u32(&mut buf, entry.size);
            push_u32(&mut buf, entry.size);
            push_u16(&mut buf, name_bytes.len() as u16);
            push_u16(&mut buf, 0); // extra field length
            push_u16(&mut buf, 0); // file comment length
            push_u16(&mut buf, 0); // disk number start
            push_u16(&mut buf, 0); // internal file attributes
            push_u32(&mut buf, 0); // external file attributes
            push_u32(&mut buf, entry.local_header_offset);
            buf.extend_from_slice(name_bytes);
        }

        let cd_size = buf.len() as u32 - cd_start;
        let entry_count = self.entries.len() as u16;

        // --- End of central directory record ---
        push_u32(&mut buf, END_OF_CENTRAL_DIR_SIG);
        push_u16(&mut buf, 0); // number of this disk
        push_u16(&mut buf, 0); // disk with start of central directory
        push_u16(&mut buf, entry_count); // entries on this disk
        push_u16(&mut buf, entry_count); // total entries
        push_u32(&mut buf, cd_size);
        push_u32(&mut buf, cd_start);
        push_u16(&mut buf, 0); // comment length

        buf
    }
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vectors() {
        // Well-known CRC-32 test vectors.
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn empty_archive_has_eocd_only() {
        let zw = ZipWriter::new();
        let bytes = zw.finish();
        assert_eq!(bytes.len(), 22, "EOCD record is exactly 22 bytes");
        assert_eq!(&bytes[0..4], &END_OF_CENTRAL_DIR_SIG.to_le_bytes());
    }

    #[test]
    fn single_entry_starts_with_local_header_signature() {
        let mut zw = ZipWriter::new();
        zw.add("hello.txt", b"hello world");
        let bytes = zw.finish();
        assert_eq!(&bytes[0..4], &LOCAL_FILE_HEADER_SIG.to_le_bytes());
        assert!(
            bytes
                .windows(4)
                .any(|w| w == CENTRAL_DIR_HEADER_SIG.to_le_bytes()),
            "must contain a central directory header"
        );
        assert!(
            bytes.ends_with(&{
                // EOCD is the final 22 bytes; just check the signature is present near the end.
                let mut v = END_OF_CENTRAL_DIR_SIG.to_le_bytes().to_vec();
                v.extend_from_slice(&bytes[bytes.len() - 18..]);
                v
            }),
            "archive must end with the EOCD record"
        );
    }

    #[test]
    fn store_method_sizes_match_data_len() {
        let mut zw = ZipWriter::new();
        let data = b"some content here";
        zw.add("a.xml", data);
        let bytes = zw.finish();
        // compression method field (offset 8..10 in local header) must be 0 (STORE).
        let method = u16::from_le_bytes([bytes[8], bytes[9]]);
        assert_eq!(method, 0, "compression method must be STORE");
        // compressed size (offset 18..22) and uncompressed size (22..26) both equal data.len().
        let compressed = u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
        let uncompressed = u32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]);
        assert_eq!(compressed, data.len() as u32);
        assert_eq!(uncompressed, data.len() as u32);
    }

    #[test]
    fn fixed_mtime_mdate_in_local_header() {
        let mut zw = ZipWriter::new();
        zw.add("a.xml", b"x");
        let bytes = zw.finish();
        let mtime = u16::from_le_bytes([bytes[10], bytes[11]]);
        let mdate = u16::from_le_bytes([bytes[12], bytes[13]]);
        assert_eq!(mtime, FIXED_DOS_TIME);
        assert_eq!(mdate, FIXED_DOS_DATE);
    }

    #[test]
    fn multiple_entries_preserve_add_order() {
        let mut zw = ZipWriter::new();
        zw.add("first.xml", b"1");
        zw.add("second.xml", b"22");
        zw.add("third.xml", b"333");
        let bytes = zw.finish();

        // Names should appear in add() order within the byte stream (local headers).
        let pos_first = find_bytes(&bytes, b"first.xml").expect("first.xml present");
        let pos_second = find_bytes(&bytes, b"second.xml").expect("second.xml present");
        let pos_third = find_bytes(&bytes, b"third.xml").expect("third.xml present");
        assert!(pos_first < pos_second);
        assert!(pos_second < pos_third);
    }

    #[test]
    fn deterministic_output_for_same_input() {
        let build = || {
            let mut zw = ZipWriter::new();
            zw.add("a.xml", b"aaa");
            zw.add("b.xml", b"bbbbb");
            zw.finish()
        };
        assert_eq!(
            build(),
            build(),
            "same entries must produce byte-identical archive"
        );
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }
}
