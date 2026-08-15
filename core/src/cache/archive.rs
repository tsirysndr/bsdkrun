//! Archive formats for `bsdkrun cache`.
//!
//! A cache entry is always a tar of the guest directory's *contents*; the
//! format only decides how that tar is wrapped. Compression happens on the
//! **host**, never in the guest: the guest already has to provide `tar` for the
//! copy, and requiring `zstd` in every image on top of that would rule out
//! most of them.

use std::io::{Read, Write};
use std::path::Path;
use std::str::FromStr;

use anyhow::{bail, Result};

pub mod estargz;

/// How a cache archive is wrapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    /// gzip — the default. Universally readable, including by plain `tar -xzf`.
    #[default]
    Gzip,
    /// zstd — markedly faster to compress and decompress at similar ratios.
    Zstd,
    /// eStargz — a gzip archive with one member per file plus a table of
    /// contents, so an individual entry can be fetched without reading the
    /// whole thing. See [`estargz`] for what that does and does not buy here.
    Estargz,
    /// A bare tar. For a store that compresses on its own, or for content that
    /// does not compress (an already-packed cache).
    None,
}

impl Compression {
    /// File extension for an archive in this format, including the `.tar`.
    pub fn extension(self) -> &'static str {
        match self {
            Compression::Gzip => "tar.gz",
            Compression::Zstd => "tar.zst",
            Compression::Estargz => "tar.estargz",
            Compression::None => "tar",
        }
    }

    /// Every accepted spelling, for CLI help and error messages.
    pub const ALL: [&'static str; 4] = ["gzip", "zstd", "estargz", "none"];
}

impl FromStr for Compression {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "gzip" | "gz" => Ok(Compression::Gzip),
            "zstd" | "zst" => Ok(Compression::Zstd),
            "estargz" | "stargz" => Ok(Compression::Estargz),
            "none" | "uncompressed" | "tar" => Ok(Compression::None),
            other => bail!(
                "unknown compression {other:?} — expected one of: {}",
                Compression::ALL.join(", ")
            ),
        }
    }
}

impl std::fmt::Display for Compression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `pad`, not `write_str`: the latter ignores the formatter's width, so
        // `{:<9}` in the `cache ls` table would silently do nothing.
        f.pad(match self {
            Compression::Gzip => "gzip",
            Compression::Zstd => "zstd",
            Compression::Estargz => "estargz",
            Compression::None => "none",
        })
    }
}

/// zstd level 3 — its default, and the point on the curve where it already
/// beats gzip on both ratio and speed. Higher levels cost far more time than
/// they save in bytes for a cache that is written as often as it is read.
const ZSTD_LEVEL: i32 = 3;

/// gzip level 6 (the default). Level 1 is what `oci.rs` uses for a throwaway
/// initramfs; a cache is written once and restored many times, so the ratio
/// matters more here.
const GZIP_LEVEL: u32 = 6;

/// Wrap `tar` bytes written to `out` in `format`, returning a writer to feed
/// the tar stream into. Call [`Sink::finish`] to flush the trailer.
pub enum Sink<W: Write> {
    Gzip(flate2::write::GzEncoder<W>),
    Zstd(zstd::stream::write::Encoder<'static, W>),
    Estargz(estargz::Writer<W>),
    Plain(W),
}

impl<W: Write> Sink<W> {
    pub fn new(out: W, format: Compression) -> Result<Self> {
        Ok(match format {
            Compression::Gzip => Sink::Gzip(flate2::write::GzEncoder::new(
                out,
                flate2::Compression::new(GZIP_LEVEL),
            )),
            Compression::Zstd => Sink::Zstd(zstd::stream::write::Encoder::new(out, ZSTD_LEVEL)?),
            Compression::Estargz => Sink::Estargz(estargz::Writer::new(out)),
            Compression::None => Sink::Plain(out),
        })
    }

    /// Finish the archive and return the underlying writer.
    pub fn finish(self) -> Result<W> {
        Ok(match self {
            Sink::Gzip(e) => e.finish()?,
            Sink::Zstd(e) => e.finish()?,
            Sink::Estargz(e) => e.finish()?,
            Sink::Plain(w) => w,
        })
    }
}

impl<W: Write> Write for Sink<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Sink::Gzip(e) => e.write(buf),
            Sink::Zstd(e) => e.write(buf),
            Sink::Estargz(e) => e.write(buf),
            Sink::Plain(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Sink::Gzip(e) => e.flush(),
            Sink::Zstd(e) => e.flush(),
            Sink::Estargz(e) => e.flush(),
            Sink::Plain(w) => w.flush(),
        }
    }
}

/// Unwrap an archive in `format` back into a plain tar stream.
///
/// estargz decodes through the gzip reader: its members concatenate into one
/// valid gzip stream, which is exactly why the format stays readable by tools
/// that know nothing about its table of contents.
pub fn reader<'a, R: Read + Send + 'a>(
    input: R,
    format: Compression,
) -> Result<Box<dyn Read + Send + 'a>> {
    Ok(match format {
        Compression::Gzip | Compression::Estargz => {
            Box::new(flate2::read::MultiGzDecoder::new(input))
        }
        Compression::Zstd => Box::new(zstd::stream::read::Decoder::new(input)?),
        Compression::None => Box::new(input),
    })
}

/// Copy a tar stream, dropping the entries eStargz adds for its own use.
///
/// The TOC and the landmark are real tar members — that is what keeps the
/// archive readable by plain `tar` — so a restore that just piped the stream
/// into the guest would leave a `stargz.index.json` and a
/// `.no.prefetch.landmark` sitting in the restored directory. Nothing else
/// strips them: the guest's `tar` does not know the format, and `--exclude` is
/// not something busybox tar can be relied on for.
pub fn strip_estargz<R: Read, W: Write>(src: R, mut out: W) -> Result<W> {
    let mut builder = tar::Builder::new(&mut out);
    let mut archive = tar::Archive::new(src);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.display().to_string();
        let name = path.trim_start_matches("./");
        if name == estargz::TOC_TAR_NAME || name == estargz::NO_PREFETCH_LANDMARK {
            continue;
        }
        let header = entry.header().clone();
        let mut body = Vec::new();
        entry.read_to_end(&mut body)?;
        builder.append(&header, &body[..])?;
    }
    builder.into_inner()?;
    Ok(out)
}

/// Infer the format from an archive's file name, for restoring an entry whose
/// metadata was lost.
pub fn from_path(path: &Path) -> Option<Compression> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".tar.estargz") {
        Some(Compression::Estargz)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(Compression::Gzip)
    } else if name.ends_with(".tar.zst") {
        Some(Compression::Zstd)
    } else if name.ends_with(".tar") {
        Some(Compression::None)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_bytes() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let body = b"hello cache\n";
        let mut header = tar::Header::new_gnu();
        header.set_path("a.txt").unwrap();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &body[..]).unwrap();
        builder.into_inner().unwrap()
    }

    fn round_trip(format: Compression) -> Vec<u8> {
        let tar = tar_bytes();
        let mut sink = Sink::new(Vec::new(), format).unwrap();
        sink.write_all(&tar).unwrap();
        let archive = sink.finish().unwrap();

        let mut out = Vec::new();
        reader(&archive[..], format)
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    fn entries_of(tar_bytes: &[u8]) -> Vec<(String, String)> {
        tar::Archive::new(tar_bytes)
            .entries()
            .unwrap()
            .map(|e| {
                let mut e = e.unwrap();
                let mut body = String::new();
                e.read_to_string(&mut body).unwrap();
                (e.path().unwrap().display().to_string(), body)
            })
            .collect()
    }

    /// Every format has to hand back the exact tar it was given — a cache that
    /// restores *almost* the right bytes is worse than one that fails.
    #[test]
    fn every_format_round_trips_the_tar_byte_for_byte() {
        for format in [Compression::Gzip, Compression::Zstd, Compression::None] {
            assert_eq!(
                entries_of(&round_trip(format)),
                vec![("a.txt".to_string(), "hello cache\n".to_string())],
                "{format} did not round-trip"
            );
        }
    }

    /// estargz is the exception: its TOC and landmark are entries *in* the tar,
    /// by design. They must never reach the guest, so [`strip_estargz`] takes
    /// them back out on the way in — this pins both halves of that.
    #[test]
    fn estargz_carries_bookkeeping_entries_that_restore_strips() {
        let raw = entries_of(&round_trip(Compression::Estargz));
        let names: Vec<_> = raw.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["a.txt", ".no.prefetch.landmark", "stargz.index.json"]
        );

        let mut stripped = Vec::new();
        strip_estargz(&round_trip(Compression::Estargz)[..], &mut stripped).unwrap();
        assert_eq!(
            entries_of(&stripped),
            vec![("a.txt".to_string(), "hello cache\n".to_string())],
        );
    }

    #[test]
    fn compression_names_and_extensions_agree() {
        for name in Compression::ALL {
            let parsed: Compression = name.parse().unwrap();
            assert_eq!(parsed.to_string(), name);
            let path = std::path::PathBuf::from(format!("c.{}", parsed.extension()));
            assert_eq!(from_path(&path), Some(parsed), "{name} extension");
        }
    }

    #[test]
    fn unknown_compression_lists_the_valid_ones() {
        let err = "brotli".parse::<Compression>().unwrap_err().to_string();
        assert!(err.contains("gzip"), "{err}");
        assert!(err.contains("estargz"), "{err}");
    }
}
