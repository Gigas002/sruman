use std::{io, time::Duration};

use ureq::{
    Agent,
    http::{HeaderMap, StatusCode, header},
};

use super::{Body, Fetch, FetchError, Head, USER_AGENT, parse_content_range};

/// [`Fetch`] on a `ureq` agent: rustls, the system proxy from the
/// environment, up to 5 redirects, and a stall timeout instead of an overall
/// one (archives are hundreds of MiB).
#[derive(Debug, Clone)]
pub struct UreqFetch {
    agent: Agent,
}

impl Default for UreqFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl UreqFetch {
    pub fn new() -> Self {
        let config = Agent::config_builder()
            .user_agent(USER_AGENT)
            .http_status_as_error(false)
            .max_redirects(5)
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_per_read(Some(Duration::from_secs(60)))
            .build();
        Self {
            agent: config.into(),
        }
    }
}

fn transport(url: &str) -> impl FnOnce(ureq::Error) -> FetchError {
    move |e| FetchError::Transport {
        url: url.to_owned(),
        message: e.to_string(),
    }
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name)?.to_str().ok()
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    header_str(headers, header::CONTENT_LENGTH)?
        .trim()
        .parse()
        .ok()
}

impl Fetch for UreqFetch {
    fn head(&self, url: &str) -> Result<Head, FetchError> {
        let response = self.agent.head(url).call().map_err(transport(url))?;
        let status = response.status();
        if !status.is_success() {
            return Err(FetchError::Status {
                url: url.to_owned(),
                status: status.as_u16(),
            });
        }
        let headers = response.headers();
        Ok(Head {
            len: content_length(headers),
            last_modified: header_str(headers, header::LAST_MODIFIED).map(str::to_owned),
        })
    }

    fn get_from(&self, url: &str, offset: u64) -> Result<Body, FetchError> {
        let mut request = self.agent.get(url);
        if offset > 0 {
            request = request.header(header::RANGE, format!("bytes={offset}-"));
        }
        let response = request.call().map_err(transport(url))?;
        let status = response.status();
        let headers = response.headers().clone();
        let bad = |message: String| FetchError::BadResponse {
            url: url.to_owned(),
            message,
        };
        let content_range = header_str(&headers, header::CONTENT_RANGE).map(parse_content_range);
        let (start, total) = match status {
            StatusCode::OK => (0, content_length(&headers)),
            StatusCode::PARTIAL_CONTENT if offset > 0 => match content_range {
                Some(Some((Some(start), total))) if start == offset => (start, total),
                other => return Err(bad(format!("unexpected Content-Range {other:?}"))),
            },
            // The cached part is already the whole file.
            StatusCode::RANGE_NOT_SATISFIABLE if offset > 0 => match content_range {
                Some(Some((None, Some(total)))) if total == offset => {
                    return Ok(Body {
                        start: offset,
                        total: Some(total),
                        reader: Box::new(io::empty()),
                    });
                }
                _ => {
                    return Err(FetchError::Status {
                        url: url.to_owned(),
                        status: status.as_u16(),
                    });
                }
            },
            status if status.is_success() => {
                return Err(bad(format!("unexpected status {}", status.as_u16())));
            }
            status => {
                return Err(FetchError::Status {
                    url: url.to_owned(),
                    status: status.as_u16(),
                });
            }
        };
        Ok(Body {
            start,
            total,
            reader: Box::new(response.into_body().into_reader()),
        })
    }
}
