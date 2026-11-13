//! Valve's runtime file lists: `mtree.txt.gz` at a build's root and
//! `usr-mtree.txt.gz` in its platform directory, and verifying a build
//! against them (`-Qk`, `-Qkk`).
//!
//! The format is BSD mtree as libarchive writes it: one entry per line,
//! `<path> key=value... [flag...]`, with paths octal-escaped (`\040` for a
//! space, `\176` for `~`).

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum MtreeError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} line {line}: {message}")]
    Parse {
        path: PathBuf,
        line: usize,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Link,
}

/// One mtree entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Relative path, unescaped, without the leading `./` (`.` for the root).
    pub path: PathBuf,
    pub kind: Kind,
    pub size: Option<u64>,
    /// Lowercase hex.
    pub sha256: Option<String>,
    /// Symlink target.
    pub link: Option<PathBuf>,
    /// Content-addressed object holding this file's data, relative to the
    /// tree root (`53/895718-1.bin`).
    pub contents: Option<PathBuf>,
    /// `ignore` or `optional`: may be absent or differ (e.g. `./var`).
    pub optional: bool,
}

impl Entry {
    /// Where the file's bytes are: its `contents` object if it has one.
    pub fn data_path(&self) -> &Path {
        self.contents.as_deref().unwrap_or(&self.path)
    }
}

/// Parses mtree text. `source` only labels errors.
pub fn parse(text: &str, source: &Path) -> Result<Vec<Entry>, MtreeError> {
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let err = |message: String| MtreeError::Parse {
            path: source.to_path_buf(),
            line: index + 1,
            message,
        };
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_ascii_whitespace();
        let raw_path = words.next().unwrap_or_default();
        if raw_path.starts_with('/') {
            return Err(err("`/set` and absolute paths are not supported".into()));
        }
        let path = relative(&unescape(raw_path).map_err(err)?).map_err(err)?;
        let mut kind = None;
        let mut entry = Entry {
            path,
            kind: Kind::File,
            size: None,
            sha256: None,
            link: None,
            contents: None,
            optional: false,
        };
        for word in words {
            match word.split_once('=') {
                Some(("type", value)) => {
                    kind = Some(match value {
                        "file" => Kind::File,
                        "dir" => Kind::Dir,
                        "link" => Kind::Link,
                        other => return Err(err(format!("unsupported type `{other}`"))),
                    });
                }
                Some(("size", value)) => {
                    entry.size = Some(
                        value
                            .parse()
                            .map_err(|_| err(format!("bad size `{value}`")))?,
                    );
                }
                Some(("sha256" | "sha256digest", value)) => {
                    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err(err(format!("bad sha256 `{value}`")));
                    }
                    entry.sha256 = Some(value.to_ascii_lowercase());
                }
                Some(("link", value)) => {
                    entry.link = Some(PathBuf::from(unescape(value).map_err(err)?))
                }
                Some(("contents", value)) => {
                    entry.contents = Some(relative(&unescape(value).map_err(err)?).map_err(err)?);
                }
                Some(_) => {} // mode, time, uid, ... are not checked
                None if matches!(word, "ignore" | "optional") => entry.optional = true,
                None => {}
            }
        }
        entry.kind = kind.ok_or_else(|| err("missing `type=`".into()))?;
        if entry.kind == Kind::Link && entry.link.is_none() {
            return Err(err("link without `link=`".into()));
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// Reads and parses a gzipped mtree file.
pub fn read_gz(path: &Path) -> Result<Vec<Entry>, MtreeError> {
    let io_err = |source| MtreeError::Io {
        path: path.to_path_buf(),
        source,
    };
    let file = fs::File::open(path).map_err(io_err)?;
    let mut text = String::new();
    flate2::read::GzDecoder::new(file)
        .read_to_string(&mut text)
        .map_err(io_err)?;
    parse(&text, path)
}

/// Decodes libarchive's octal escapes (`\040`, `\176`, `\\`).
fn unescape(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            let digits = bytes
                .get(i + 1..i + 4)
                .filter(|d| d.iter().all(|b| (b'0'..=b'7').contains(b)));
            if let Some(digits) = digits {
                let value = digits
                    .iter()
                    .fold(0u32, |acc, b| acc * 8 + u32::from(b - b'0'));
                out.push(u8::try_from(value).map_err(|_| format!("bad escape in `{text}`"))?);
                i += 4;
                continue;
            }
            if bytes.get(i + 1) == Some(&b'\\') {
                out.push(b'\\');
                i += 2;
                continue;
            }
            return Err(format!("bad escape in `{text}`"));
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).map_err(|_| format!("`{text}` is not UTF-8"))
}

/// `./a/b` → `a/b`; refuses anything that could leave the tree.
fn relative(text: &str) -> Result<PathBuf, String> {
    let trimmed = text.strip_prefix("./").unwrap_or(text);
    if trimmed == "." || trimmed.is_empty() {
        return Ok(PathBuf::from("."));
    }
    let path = Path::new(trimmed);
    if path
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        Ok(path.to_path_buf())
    } else {
        Err(format!("path `{text}` leaves the tree"))
    }
}

/// How thoroughly to verify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// `-Qk`: every listed file, directory and link exists; file sizes match.
    Presence,
    /// `-Qkk`: also SHA-256 of every file.
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    Missing(PathBuf),
    WrongType(PathBuf),
    WrongSize {
        path: PathBuf,
        expected: u64,
        found: u64,
    },
    WrongLink {
        path: PathBuf,
        expected: PathBuf,
        found: PathBuf,
    },
    WrongHash(PathBuf),
    Unreadable {
        path: PathBuf,
        message: String,
    },
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(path) => write!(f, "{}: missing", path.display()),
            Self::WrongType(path) => write!(f, "{}: wrong file type", path.display()),
            Self::WrongSize {
                path,
                expected,
                found,
            } => {
                write!(f, "{}: size {found}, expected {expected}", path.display())
            }
            Self::WrongLink {
                path,
                expected,
                found,
            } => write!(
                f,
                "{}: links to {}, expected {}",
                path.display(),
                found.display(),
                expected.display()
            ),
            Self::WrongHash(path) => write!(f, "{}: content differs (sha256)", path.display()),
            Self::Unreadable { path, message } => write!(f, "{}: {message}", path.display()),
        }
    }
}

/// Result of checking a tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Entries checked.
    pub checked: usize,
    pub problems: Vec<Problem>,
}

/// Checks `root` against `entries`. With `objects_only`, only regular files
/// with data are checked (a platform's `files/` tree, which pressure-vessel assembles
/// from the objects; its directories and links aren't all materialized).
/// Files sharing a `contents` object are checked once.
pub fn verify(root: &Path, entries: &[Entry], depth: Depth, objects_only: bool) -> Report {
    let mut report = Report::default();
    let mut seen = BTreeSet::new();
    for entry in entries {
        // A platform tree only stores files with data: pressure-vessel
        // creates the empty ones (no digest) itself.
        let empty = entry.size == Some(0) && entry.sha256.is_none();
        if entry.optional || (objects_only && (entry.kind != Kind::File || empty)) {
            continue;
        }
        let data = entry.data_path();
        if entry.kind == Kind::File && !seen.insert(data.to_path_buf()) {
            continue;
        }
        report.checked += 1;
        let path = root.join(data);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                report.problems.push(Problem::Missing(entry.path.clone()));
                continue;
            }
            Err(e) => {
                report.problems.push(Problem::Unreadable {
                    path: entry.path.clone(),
                    message: e.to_string(),
                });
                continue;
            }
        };
        match entry.kind {
            Kind::Dir if !meta.is_dir() => {
                report.problems.push(Problem::WrongType(entry.path.clone()))
            }
            Kind::Dir => {}
            Kind::Link => {
                if !meta.file_type().is_symlink() {
                    report.problems.push(Problem::WrongType(entry.path.clone()));
                } else if let (Ok(found), Some(expected)) = (fs::read_link(&path), &entry.link)
                    && &found != expected
                {
                    report.problems.push(Problem::WrongLink {
                        path: entry.path.clone(),
                        expected: expected.clone(),
                        found,
                    });
                }
            }
            Kind::File => {
                if !meta.is_file() {
                    report.problems.push(Problem::WrongType(entry.path.clone()));
                    continue;
                }
                if let Some(expected) = entry.size
                    && meta.len() != expected
                {
                    report.problems.push(Problem::WrongSize {
                        path: entry.path.clone(),
                        expected,
                        found: meta.len(),
                    });
                    continue;
                }
                if depth == Depth::Content
                    && let Some(expected) = &entry.sha256
                {
                    match sha256_file(&path) {
                        Ok(found) if &found == expected => {}
                        Ok(_) => report.problems.push(Problem::WrongHash(entry.path.clone())),
                        Err(e) => report.problems.push(Problem::Unreadable {
                            path: entry.path.clone(),
                            message: e.to_string(),
                        }),
                    }
                }
            }
        }
    }
    report
}

/// Lowercase hex SHA-256 of a file, streamed.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}
