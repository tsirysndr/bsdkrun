//! An eStargz writer.
//!
//! eStargz is a tar.gz laid out so an individual file can be fetched without
//! reading the archive: every entry's payload begins its own gzip member, a
//! `stargz.index.json` table of contents records each member's byte offset, and
//! a 51-byte footer at the very end says where that TOC begins. Because gzip
//! members concatenate into one valid gzip stream, the result is still an
//! ordinary `.tar.gz` to anything that does not care — `tar -xzf` reads it.
//!
//! **What it buys here, today: interoperability, not speed.** `bsdkrun cache
//! restore` unpacks the whole tree, so it never seeks, and the per-member
//! framing makes the archive slightly *larger* than plain gzip. The reason to
//! write it is that the artifact is the same one containerd, stargz-snapshotter
//! and `ctr-remote` consume — a cache saved here can be served to a lazy-pulling
//! runtime, and a future partial restore has the offsets it would need.
//!
//! Spec: <https://github.com/containerd/stargz-snapshotter/blob/main/docs/estargz.md>

use std::io::{Read, Seek, SeekFrom, Write};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Tar entry name of the table of contents.
pub(super) const TOC_TAR_NAME: &str = "stargz.index.json";

/// Landmark marking "this archive has no prefetch range". The spec requires one
/// of the two landmarks to be present; without it a verifying reader rejects the
/// archive, which is exactly the interoperability we are here for.
pub(super) const NO_PREFETCH_LANDMARK: &str = ".no.prefetch.landmark";

/// The footer is a gzip member carrying no data, whose Extra field points at the
/// TOC. Its length is fixed by the spec, and readers seek to `len - 51`.
pub const FOOTER_SIZE: usize = 51;

/// TOC schema version.
const TOC_VERSION: u32 = 1;

/// One entry in `stargz.index.json`.
///
/// Field names are the wire format's, so they are camelCase rather than Rust's
/// convention. Absent fields are omitted rather than sent as null — a reader
/// distinguishes "no link" from `"linkName": ""`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct TocEntry {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "is_zero_u64", default)]
    size: u64,
    #[serde(rename = "modtime", skip_serializing_if = "String::is_empty", default)]
    mod_time: String,
    #[serde(rename = "linkName", skip_serializing_if = "String::is_empty", default)]
    link_name: String,
    mode: i64,
    #[serde(skip_serializing_if = "is_zero_u64", default)]
    uid: u64,
    #[serde(skip_serializing_if = "is_zero_u64", default)]
    gid: u64,
    #[serde(rename = "userName", skip_serializing_if = "String::is_empty", default)]
    user_name: String,
    #[serde(
        rename = "groupName",
        skip_serializing_if = "String::is_empty",
        default
    )]
    group_name: String,
    #[serde(rename = "devMajor", skip_serializing_if = "is_zero_u64", default)]
    dev_major: u64,
    #[serde(rename = "devMinor", skip_serializing_if = "is_zero_u64", default)]
    dev_minor: u64,
    /// Offset of the gzip member holding this entry's payload.
    #[serde(skip_serializing_if = "is_zero_u64", default)]
    offset: u64,
    /// `sha256:…` over the file's contents.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    digest: String,
}

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Toc {
    version: u32,
    entries: Vec<TocEntry>,
}

/// Buffers the incoming tar, then converts it on [`finish`](Writer::finish).
///
/// The conversion has to read the tar back to find entry boundaries, so it
/// cannot be done in a single forward pass over a `Write`. Staging goes to a
/// temp *file*, not memory, so a multi-gigabyte cache costs disk rather than
/// RAM — and that file is the only extra cost of choosing this format.
pub struct Writer<W: Write> {
    staging: Option<tempfile::NamedTempFile>,
    out: Option<W>,
}

impl<W: Write> Writer<W> {
    pub fn new(out: W) -> Self {
        Writer {
            staging: None,
            out: Some(out),
        }
    }

    fn staging(&mut self) -> std::io::Result<&mut tempfile::NamedTempFile> {
        if self.staging.is_none() {
            self.staging = Some(tempfile::NamedTempFile::new()?);
        }
        Ok(self.staging.as_mut().expect("just created"))
    }

    /// Convert the staged tar into eStargz and return the underlying writer.
    pub fn finish(mut self) -> Result<W> {
        let out = self.out.take().expect("finish called once");
        let Some(mut staging) = self.staging.take() else {
            // Nothing was ever written. Emit a well-formed empty archive so a
            // reader gets an empty tar rather than a truncated file.
            return build(std::io::empty(), out);
        };
        staging.flush()?;
        staging.as_file_mut().seek(SeekFrom::Start(0))?;
        let file = staging.reopen().context("reopening the staged tar")?;
        build(file, out)
    }
}

impl<W: Write> Write for Writer<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.staging()?.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.staging.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

/// Counts bytes on their way out, so a TOC offset is just `counter.n`.
struct Counting<W: Write> {
    inner: W,
    n: u64,
}

impl<W: Write> Write for Counting<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.n += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Write one gzip member containing exactly `body`, and return the output
/// offset it started at.
fn member<W: Write>(out: &mut Counting<W>, body: &[u8]) -> Result<u64> {
    let at = out.n;
    let mut gz = flate2::write::GzEncoder::new(&mut *out, flate2::Compression::default());
    gz.write_all(body)?;
    gz.finish()?;
    Ok(at)
}

/// Read a tar from `src` and write the eStargz form to `out`.
///
/// Iteration is over the *raw* entries, so GNU long-name and long-link pseudo
/// entries arrive as themselves and are re-emitted byte for byte. That keeps the
/// decompressed stream identical to the input — the alternative, rebuilding
/// headers through `tar::Builder`, both loses the original bytes and terminates
/// the archive early, because its `into_inner` writes the end-of-archive blocks.
fn build<R: Read, W: Write>(src: R, out: W) -> Result<W> {
    let mut out = Counting { inner: out, n: 0 };
    let mut toc = Toc {
        version: TOC_VERSION,
        entries: Vec::new(),
    };

    // A GNU long name/link arrives as a pseudo entry *before* the entry it
    // describes; hold it until that entry shows up so the TOC records the real
    // path rather than the 100-byte truncation in the header.
    let mut pending_name: Option<String> = None;
    let mut pending_link: Option<String> = None;

    let mut archive = tar::Archive::new(src);
    let entries = archive
        .entries()
        .context("reading the staged tar")?
        .raw(true);
    for entry in entries {
        let mut entry = entry.context("reading a tar entry")?;
        let header = entry.header().clone();
        let size = header.size().unwrap_or(0);
        let entry_type = header.entry_type();

        // The header block, verbatim, in its own member.
        member(&mut out, header.as_bytes())?;

        let mut payload = Vec::with_capacity(size as usize);
        entry
            .read_to_end(&mut payload)
            .context("reading a tar entry body")?;

        let offset = if size > 0 {
            let mut padded = payload.clone();
            pad_to_block(&mut padded);
            member(&mut out, &padded)?
        } else {
            out.n
        };

        // Pseudo entries carry a name, not a file: stash and move on.
        if entry_type == tar::EntryType::GNULongName {
            pending_name = Some(cstr(&payload));
            continue;
        }
        if entry_type == tar::EntryType::GNULongLink {
            pending_link = Some(cstr(&payload));
            continue;
        }

        let name = match pending_name.take() {
            Some(n) => n,
            None => header
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        };
        let link = match pending_link.take() {
            Some(l) => l,
            None => header
                .link_name()
                .ok()
                .flatten()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        };

        toc.entries.push(TocEntry {
            name: normalize(&name),
            kind: kind_of(&header).to_string(),
            size,
            mod_time: rfc3339(header.mtime().unwrap_or(0)),
            link_name: normalize(&link),
            mode: header.mode().unwrap_or(0) as i64,
            uid: header.uid().unwrap_or(0),
            gid: header.gid().unwrap_or(0),
            user_name: header.username().ok().flatten().unwrap_or("").to_string(),
            group_name: header.groupname().ok().flatten().unwrap_or("").to_string(),
            dev_major: header.device_major().ok().flatten().unwrap_or(0) as u64,
            dev_minor: header.device_minor().ok().flatten().unwrap_or(0) as u64,
            offset,
            digest: if payload.is_empty() {
                String::new()
            } else {
                format!("sha256:{:x}", Sha256::digest(&payload))
            },
        });
    }

    append_generated(&mut out, &mut toc, NO_PREFETCH_LANDMARK, &[0u8])?;

    // The TOC, last, in a member of its own — the footer points at where it
    // starts. Its header and its JSON go in the *same* member, unlike a file's:
    // a reader takes one gzip member at the footer's offset, calls tar.Next()
    // for the header and then decodes the JSON from the same stream. Splitting
    // them the way regular entries are split ends that stream after the header,
    // and the TOC decode fails with "unexpected EOF".
    let json = serde_json::to_vec(&toc).context("serializing the estargz TOC")?;
    let mut toc_blocks = plain_header(TOC_TAR_NAME, json.len() as u64)?.to_vec();
    toc_blocks.extend_from_slice(&json);
    pad_to_block(&mut toc_blocks);
    let toc_offset = member(&mut out, &toc_blocks)?;

    // A tar ends with two zero blocks; without them `tar -xzf` warns.
    member(&mut out, &[0u8; 1024])?;

    out.write_all(&footer(toc_offset))?;
    out.flush()?;
    Ok(out.inner)
}

/// A 512-byte ustar header for a short-named regular file we generate.
fn plain_header(name: &str, size: u64) -> Result<[u8; 512]> {
    let mut h = tar::Header::new_gnu();
    h.set_path(name)
        .with_context(|| format!("naming the generated entry {name}"))?;
    h.set_size(size);
    h.set_mode(0o644);
    h.set_entry_type(tar::EntryType::Regular);
    h.set_cksum();
    Ok(*h.as_bytes())
}

/// Append a small file bsdkrun generates (the landmark) as a real tar entry, so
/// the archive stays a valid tar, and record it in the TOC.
fn append_generated<W: Write>(
    out: &mut Counting<W>,
    toc: &mut Toc,
    name: &str,
    body: &[u8],
) -> Result<()> {
    member(out, &plain_header(name, body.len() as u64)?)?;
    let mut padded = body.to_vec();
    pad_to_block(&mut padded);
    let offset = member(out, &padded)?;

    toc.entries.push(TocEntry {
        name: name.to_string(),
        kind: "reg".to_string(),
        size: body.len() as u64,
        mod_time: String::new(),
        link_name: String::new(),
        mode: 0o644,
        uid: 0,
        gid: 0,
        user_name: String::new(),
        group_name: String::new(),
        dev_major: 0,
        dev_minor: 0,
        offset,
        digest: format!("sha256:{:x}", Sha256::digest(body)),
    });
    Ok(())
}

/// A GNU pseudo entry's body is the name, NUL-terminated.
fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// The 51-byte footer: an empty gzip member whose Extra field is the subfield
/// `S G` carrying `%016xSTARGZ`, the TOC's offset in hex.
fn footer(toc_offset: u64) -> Vec<u8> {
    let payload = format!("{toc_offset:016x}STARGZ");
    debug_assert_eq!(payload.len(), 22);
    let mut extra = Vec::with_capacity(4 + payload.len());
    extra.push(b'S');
    extra.push(b'G');
    extra.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    extra.extend_from_slice(payload.as_bytes());

    let mut buf = Vec::new();
    let gz = flate2::GzBuilder::new()
        .extra(extra)
        .write(&mut buf, flate2::Compression::none());
    gz.finish().expect("writing to a Vec cannot fail");
    buf
}

/// Read a footer back, returning the TOC offset it points at. The inverse of
/// [`footer`], and what a reader does first.
pub fn parse_footer(bytes: &[u8]) -> Option<u64> {
    if bytes.len() != FOOTER_SIZE {
        return None;
    }
    // Locate the subfield payload rather than assuming a fixed position: the
    // gzip header before it is fixed-width today, but reading it out by pattern
    // keeps this honest if flate2 ever emits an OS byte or MTIME differently.
    let marker = bytes.windows(6).position(|w| w == b"STARGZ")?;
    let hex = bytes.get(marker.checked_sub(16)?..marker)?;
    u64::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()
}

fn pad_to_block(buf: &mut Vec<u8>) {
    let rem = buf.len() % 512;
    if rem != 0 {
        buf.resize(buf.len() + (512 - rem), 0);
    }
}

/// eStargz spells tar's type flags out in words.
fn kind_of(header: &tar::Header) -> &'static str {
    use tar::EntryType::*;
    match header.entry_type() {
        Directory => "dir",
        Symlink => "symlink",
        Link => "hardlink",
        Char => "char",
        Block => "block",
        Fifo => "fifo",
        _ => "reg",
    }
}

/// TOC paths are relative and unprefixed; tar writes them as `./foo`.
fn normalize(path: &str) -> String {
    path.trim_start_matches("./").to_string()
}

/// Format unix seconds as RFC 3339 UTC, which is what the TOC's `modtime` is.
///
/// Hand-rolled rather than pulling in a date library for one field: this is the
/// civil-from-days algorithm, exact for any date after 1970.
fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    // Days since 1970-01-01 -> y/m/d (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tar() -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (name, body) in [
            ("small.txt", "hello\n".as_bytes()),
            ("nested/deep/file.bin", &[0u8, 1, 2, 255, 254][..]),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_mtime(1_760_000_000);
            h.set_entry_type(tar::EntryType::Regular);
            b.append_data(&mut h, name, body).unwrap();
        }
        b.into_inner().unwrap()
    }

    fn build_sample() -> Vec<u8> {
        let mut w = Writer::new(Vec::new());
        w.write_all(&sample_tar()).unwrap();
        w.finish().unwrap()
    }

    /// The whole point of concatenated gzip members: anything that can read a
    /// .tar.gz can read this, TOC or no TOC.
    #[test]
    fn a_plain_gzip_reader_sees_the_original_files() {
        let archive = build_sample();
        let mut tar = Vec::new();
        flate2::read::MultiGzDecoder::new(&archive[..])
            .read_to_end(&mut tar)
            .unwrap();

        let mut found = Vec::new();
        for e in tar::Archive::new(&tar[..]).entries().unwrap() {
            let mut e = e.unwrap();
            let name = e.path().unwrap().display().to_string();
            let mut body = Vec::new();
            e.read_to_end(&mut body).unwrap();
            found.push((name, body));
        }
        assert_eq!(found[0].0, "small.txt");
        assert_eq!(found[0].1, b"hello\n");
        assert_eq!(found[1].0, "nested/deep/file.bin");
        assert_eq!(found[1].1, vec![0u8, 1, 2, 255, 254]);
    }

    #[test]
    fn the_footer_is_the_size_the_spec_fixes() {
        assert_eq!(footer(0).len(), FOOTER_SIZE);
        assert_eq!(footer(u32::MAX as u64).len(), FOOTER_SIZE);
    }

    #[test]
    fn the_footer_round_trips_the_toc_offset() {
        for off in [0u64, 1, 512, 123_456, 0xdead_beef] {
            assert_eq!(parse_footer(&footer(off)), Some(off), "offset {off}");
        }
    }

    /// A reader seeks to `len - 51`, reads the offset, and gunzips there. If the
    /// offset is off by even one byte it lands mid-member and gets nothing —
    /// so follow it and check the TOC really is where it says.
    #[test]
    fn the_toc_is_where_the_footer_says_it_is() {
        let archive = build_sample();
        let toc_offset =
            parse_footer(&archive[archive.len() - FOOTER_SIZE..]).expect("footer parses") as usize;

        let mut tar_bytes = Vec::new();
        flate2::read::MultiGzDecoder::new(&archive[toc_offset..])
            .read_to_end(&mut tar_bytes)
            .unwrap();
        let mut entries = tar::Archive::new(&tar_bytes[..]);
        let mut e = entries.entries().unwrap().next().unwrap().unwrap();
        assert_eq!(e.path().unwrap().display().to_string(), TOC_TAR_NAME);

        let mut json = String::new();
        e.read_to_string(&mut json).unwrap();
        let toc: Toc = serde_json::from_str(&json).unwrap();
        assert_eq!(toc.version, TOC_VERSION);
        let names: Vec<_> = toc.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"small.txt"), "{names:?}");
        assert!(names.contains(&NO_PREFETCH_LANDMARK), "{names:?}");
    }

    /// Each entry's `offset` must point at the gzip member holding its *payload*
    /// — not its tar header. Getting this wrong yields 512 bytes of header where
    /// the file contents should be, which is the mistake the format invites.
    #[test]
    fn each_entry_offset_lands_on_its_own_payload() {
        let archive = build_sample();
        let toc_offset =
            parse_footer(&archive[archive.len() - FOOTER_SIZE..]).expect("footer parses") as usize;
        let mut json_tar = Vec::new();
        flate2::read::MultiGzDecoder::new(&archive[toc_offset..])
            .read_to_end(&mut json_tar)
            .unwrap();
        let mut e = tar::Archive::new(&json_tar[..]);
        let mut first = e.entries().unwrap().next().unwrap().unwrap();
        let mut json = String::new();
        first.read_to_string(&mut json).unwrap();
        let toc: Toc = serde_json::from_str(&json).unwrap();

        for want in [
            ("small.txt", &b"hello\n"[..]),
            ("nested/deep/file.bin", &[0u8, 1, 2, 255, 254][..]),
        ] {
            let entry = toc
                .entries
                .iter()
                .find(|e| e.name == want.0)
                .unwrap_or_else(|| panic!("{} missing from the TOC", want.0));
            let mut got = vec![0u8; entry.size as usize];
            let mut gz = flate2::read::GzDecoder::new(&archive[entry.offset as usize..]);
            gz.read_exact(&mut got).unwrap();
            assert_eq!(got, want.1, "{} payload at its TOC offset", want.0);
            assert_eq!(entry.digest, format!("sha256:{:x}", Sha256::digest(want.1)));
        }
    }

    #[test]
    fn modtimes_are_rfc3339_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_760_000_000), "2025-10-09T08:53:20Z");
    }

    /// An empty input still has to produce something a reader accepts, rather
    /// than a zero-byte file that looks like a failed upload.
    #[test]
    fn an_empty_archive_is_still_well_formed() {
        let out = Writer::new(Vec::new()).finish().unwrap();
        assert!(parse_footer(&out[out.len() - FOOTER_SIZE..]).is_some());
        let mut tar = Vec::new();
        flate2::read::MultiGzDecoder::new(&out[..])
            .read_to_end(&mut tar)
            .unwrap();
        // The landmark and the TOC, and nothing else.
        let names: Vec<_> = tar::Archive::new(&tar[..])
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        assert_eq!(names, vec![NO_PREFETCH_LANDMARK, TOC_TAR_NAME]);
    }
}
