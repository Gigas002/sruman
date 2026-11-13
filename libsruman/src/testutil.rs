//! Test helpers shared by the `download` modules: an in-memory [`Fetch`]
//! and a `.tar.xz` builder that can write entries a normal tar writer
//! refuses (`..` paths, escaping links, devices).

use std::{
    collections::BTreeMap,
    io::{self, Read},
    sync::Mutex,
};

use crate::fetch::{Body, Fetch, FetchError, Head};

/// One file on the [`MemFetch`] server.
#[derive(Debug, Clone, Default)]
pub(crate) struct MemFile {
    pub data: Vec<u8>,
    /// Fail the body with an I/O error after this many bytes, once.
    pub cut_after: Option<usize>,
    /// Answer ranged requests with the whole file.
    pub ignore_range: bool,
}

/// An in-memory server that logs every request.
#[derive(Default)]
pub(crate) struct MemFetch {
    files: Mutex<BTreeMap<String, MemFile>>,
    log: Mutex<Vec<String>>,
}

impl MemFetch {
    pub fn put(&self, url: &str, data: impl Into<Vec<u8>>) {
        self.put_file(
            url,
            MemFile {
                data: data.into(),
                ..MemFile::default()
            },
        );
    }

    pub fn put_file(&self, url: &str, file: MemFile) {
        self.files.lock().unwrap().insert(url.to_owned(), file);
    }

    pub fn remove(&self, url: &str) {
        self.files.lock().unwrap().remove(url);
    }

    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }

    fn find(&self, url: &str) -> Result<MemFile, FetchError> {
        self.files
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .ok_or_else(|| FetchError::Status {
                url: url.to_owned(),
                status: 404,
            })
    }
}

/// Yields `data`, then fails.
struct Cut(io::Cursor<Vec<u8>>);

impl Read for Cut {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buf)? {
            0 => Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "connection cut",
            )),
            n => Ok(n),
        }
    }
}

impl Fetch for MemFetch {
    fn head(&self, url: &str) -> Result<Head, FetchError> {
        self.log.lock().unwrap().push(format!("HEAD {url}"));
        Ok(Head {
            len: Some(self.find(url)?.data.len() as u64),
            last_modified: None,
        })
    }

    fn get_from(&self, url: &str, offset: u64) -> Result<Body, FetchError> {
        let log = if offset > 0 {
            format!("GET {url} @{offset}")
        } else {
            format!("GET {url}")
        };
        self.log.lock().unwrap().push(log);
        let file = self.find(url)?;
        let total = file.data.len() as u64;
        let start = if file.ignore_range { 0 } else { offset };
        if start > total {
            return Err(FetchError::Status {
                url: url.to_owned(),
                status: 416,
            });
        }
        let rest = file.data[start as usize..].to_vec();
        let reader: Box<dyn Read + Send> = match file.cut_after {
            Some(cut) => {
                if let Some(stored) = self.files.lock().unwrap().get_mut(url) {
                    stored.cut_after = None;
                }
                Box::new(Cut(io::Cursor::new(rest[..cut.min(rest.len())].to_vec())))
            }
            None => Box::new(io::Cursor::new(rest)),
        };
        Ok(Body {
            start,
            total: Some(total),
            reader,
        })
    }
}

/// One entry for [`tar_xz`].
pub(crate) enum TarEntry<'a> {
    Dir(&'a str),
    File(&'a str, &'a [u8], u32),
    Symlink(&'a str, &'a str),
    Hardlink(&'a str, &'a str),
    Fifo(&'a str),
}

/// A tar stream with raw headers (no path validation).
pub(crate) fn tar(entries: &[TarEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries {
        let mut header = tar::Header::new_old();
        let (path, kind, data, mode, link): (&str, tar::EntryType, &[u8], u32, &str) = match entry {
            TarEntry::Dir(p) => (p, tar::EntryType::Directory, b"", 0o755, ""),
            TarEntry::File(p, d, m) => (p, tar::EntryType::Regular, d, *m, ""),
            TarEntry::Symlink(p, l) => (p, tar::EntryType::Symlink, b"", 0o777, l),
            TarEntry::Hardlink(p, l) => (p, tar::EntryType::Link, b"", 0o644, l),
            TarEntry::Fifo(p) => (p, tar::EntryType::Fifo, b"", 0o644, ""),
        };
        let old = header.as_old_mut();
        old.name[..path.len()].copy_from_slice(path.as_bytes());
        old.linkname[..link.len()].copy_from_slice(link.as_bytes());
        header.set_entry_type(kind);
        header.set_mode(mode);
        header.set_size(data.len() as u64);
        header.set_mtime(1_700_000_000);
        header.set_cksum();
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.resize(out.len() + 1024, 0);
    out
}

pub(crate) fn xz(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut writer =
        lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1)).unwrap();
    writer.write_all(data).unwrap();
    writer.finish().unwrap()
}

pub(crate) fn tar_xz(entries: &[TarEntry]) -> Vec<u8> {
    xz(&tar(entries))
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
