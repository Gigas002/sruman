//! HTTP access behind a trait, so callers can bring their own client.
//!
//! sruman only ever does plain `GET`/`HEAD` on public files under
//! `repo.steampowered.com`, so [`Fetch`] is small: [`Fetch::head`] for
//! sizes, [`Fetch::get_from`] for streaming (with `Range` to resume a
//! download), and [`Fetch::get`] for small text files with a size cap.
//! [`UreqFetch`] is the default implementation; gaki can implement the trait
//! on its own `reqwest` client (proxy and all) and use only the `download`
//! feature.
//!
//! No request carries anything about the user: the User-Agent is
//! `sruman/<version>` and there are no cookies or credentials.

use std::io::{self, Read};

#[cfg(feature = "ureq")]
mod ureq_fetch;
#[cfg(feature = "ureq")]
pub use ureq_fetch::UreqFetch;

#[cfg(test)]
mod tests;

/// The User-Agent every built-in request sends.
pub const USER_AGENT: &str = concat!("sruman/", env!("CARGO_PKG_VERSION"));

/// Cap for [`Fetch::get`] callers reading index files (`SHA256SUMS`,
/// manifests, pointer files); the largest real one is under 200 KiB.
pub const SMALL_FILE_LIMIT: u64 = 16 * 1024 * 1024;

/// What a `HEAD` tells.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Head {
    /// `Content-Length`, if the server sent one.
    pub len: Option<u64>,
    /// `Last-Modified`, verbatim.
    pub last_modified: Option<String>,
}

/// A response body being streamed.
pub struct Body {
    /// Where `reader` starts in the file: the requested offset when the
    /// server honored the `Range`, or 0 when it sent the whole file (the
    /// caller then starts over).
    pub start: u64,
    /// The whole file's size, if known.
    pub total: Option<u64>,
    pub reader: Box<dyn Read + Send>,
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Body")
            .field("start", &self.start)
            .field("total", &self.total)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("{url}: HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("{url}: {message}")]
    Transport { url: String, message: String },
    #[error("{url}: larger than {limit} bytes")]
    TooLarge { url: String, limit: u64 },
    #[error("{url}: bad response: {message}")]
    BadResponse { url: String, message: String },
    #[error("{url}: {source}")]
    Io {
        url: String,
        #[source]
        source: io::Error,
    },
}

impl FetchError {
    /// A 404 (or 410): the file isn't there, which some callers expect.
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::Status {
                status: 404 | 410,
                ..
            }
        )
    }
}

/// Plain HTTP(S) reads. Implementations follow redirects, and must return
/// [`FetchError::Status`] for any non-success status.
pub trait Fetch: Send + Sync {
    fn head(&self, url: &str) -> Result<Head, FetchError>;

    /// Streams `url` from byte `offset` (a `Range: bytes=<offset>-` request
    /// when `offset > 0`). See [`Body::start`] for servers that ignore it.
    fn get_from(&self, url: &str, offset: u64) -> Result<Body, FetchError>;

    /// Reads all of `url`, failing with [`FetchError::TooLarge`] past `limit`.
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, FetchError> {
        let body = self.get_from(url, 0)?;
        if body.total.is_some_and(|total| total > limit) {
            return Err(FetchError::TooLarge {
                url: url.to_owned(),
                limit,
            });
        }
        let mut data = Vec::new();
        body.reader
            .take(limit.saturating_add(1))
            .read_to_end(&mut data)
            .map_err(|source| FetchError::Io {
                url: url.to_owned(),
                source,
            })?;
        if data.len() as u64 > limit {
            return Err(FetchError::TooLarge {
                url: url.to_owned(),
                limit,
            });
        }
        Ok(data)
    }

    /// [`Fetch::get`] as UTF-8 text.
    fn get_text(&self, url: &str, limit: u64) -> Result<String, FetchError> {
        String::from_utf8(self.get(url, limit)?).map_err(|_| FetchError::BadResponse {
            url: url.to_owned(),
            message: "not UTF-8 text".into(),
        })
    }
}

/// Parses `Content-Range: bytes <start>-<end>/<total|*>` (or `bytes */<total>`)
/// into `(start, total)`; `start` is `None` for the unsatisfied form.
pub fn parse_content_range(value: &str) -> Option<(Option<u64>, Option<u64>)> {
    let rest = value.trim().strip_prefix("bytes")?.trim_start();
    let (range, total) = rest.split_once('/')?;
    let total = match total.trim() {
        "*" => None,
        n => Some(n.parse().ok()?),
    };
    let start = match range.trim() {
        "*" => None,
        range => {
            let (start, end) = range.split_once('-')?;
            let (start, end): (u64, u64) = (start.parse().ok()?, end.parse().ok()?);
            if end < start {
                return None;
            }
            Some(start)
        }
    };
    Some((start, total))
}
