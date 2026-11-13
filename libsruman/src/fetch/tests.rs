use super::*;

#[path = "../../tests/support/http.rs"]
mod http;

#[test]
fn content_range_forms() {
    assert_eq!(
        parse_content_range("bytes 10-19/20"),
        Some((Some(10), Some(20)))
    );
    assert_eq!(parse_content_range("bytes 0-0/*"), Some((Some(0), None)));
    assert_eq!(parse_content_range("bytes */20"), Some((None, Some(20))));
    for bad in [
        "",
        "bytes",
        "bytes 5-1/9",
        "items 0-1/2",
        "bytes a-b/c",
        "bytes 0-1",
    ] {
        assert_eq!(parse_content_range(bad), None, "{bad}");
    }
}

#[test]
fn not_found_is_recognized() {
    let e = FetchError::Status {
        url: "u".into(),
        status: 404,
    };
    assert!(e.is_not_found());
    let e = FetchError::Status {
        url: "u".into(),
        status: 500,
    };
    assert!(!e.is_not_found());
}

/// A `Fetch` that only implements the two required methods, to test the
/// provided ones.
struct Canned(Vec<u8>, Option<u64>);

impl Fetch for Canned {
    fn head(&self, _: &str) -> Result<Head, FetchError> {
        Ok(Head::default())
    }

    fn get_from(&self, _: &str, _: u64) -> Result<Body, FetchError> {
        Ok(Body {
            start: 0,
            total: self.1,
            reader: Box::new(io::Cursor::new(self.0.clone())),
        })
    }
}

#[test]
fn get_enforces_the_limit_with_or_without_a_length() {
    assert_eq!(
        Canned(b"abc".to_vec(), Some(3)).get("u", 3).unwrap(),
        b"abc"
    );
    assert!(matches!(
        Canned(b"abcd".to_vec(), None).get("u", 3),
        Err(FetchError::TooLarge { .. })
    ));
    assert!(matches!(
        Canned(Vec::new(), Some(4)).get("u", 3),
        Err(FetchError::TooLarge { .. })
    ));
    assert!(matches!(
        Canned(vec![0xff], None).get_text("u", 3),
        Err(FetchError::BadResponse { .. })
    ));
}

#[cfg(feature = "ureq")]
mod ureq {
    use super::{http::*, *};

    fn read(body: Body) -> Vec<u8> {
        let mut data = Vec::new();
        let mut reader = body.reader;
        reader.read_to_end(&mut data).unwrap();
        data
    }

    #[test]
    fn get_head_and_user_agent() {
        let server = Server::start();
        server.put_bytes("/a.txt", "hello");
        let fetch = UreqFetch::new();
        assert_eq!(fetch.get_text(&server.url("/a.txt"), 100).unwrap(), "hello");
        let head = fetch.head(&server.url("/a.txt")).unwrap();
        assert_eq!(head.len, Some(5));
        assert_eq!(
            head.last_modified.as_deref(),
            Some("Thu, 12 Nov 2026 10:00:00 GMT")
        );
        let log = server.log();
        assert_eq!(log.len(), 2);
        assert!(
            log.iter()
                .all(|r| r.user_agent.as_deref() == Some(USER_AGENT)),
            "{log:?}"
        );
        assert_eq!(log[1].method, "HEAD");
    }

    #[test]
    fn errors_carry_status_and_url() {
        let server = Server::start();
        let fetch = UreqFetch::new();
        let url = server.url("/missing");
        let err = fetch.get(&url, 10).unwrap_err();
        assert!(err.is_not_found(), "{err}");
        assert_eq!(err.to_string(), format!("{url}: HTTP 404"));
        assert!(fetch.head(&url).unwrap_err().is_not_found());

        server.put_bytes("/big", vec![0; 100]);
        assert!(matches!(
            fetch.get(&server.url("/big"), 10),
            Err(FetchError::TooLarge { .. })
        ));

        let closed = format!("http://127.0.0.1:{}/x", {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        });
        assert!(matches!(
            fetch.get(&closed, 10),
            Err(FetchError::Transport { .. })
        ));
    }

    #[test]
    fn ranges_resume_or_restart() {
        let server = Server::start();
        let data: Vec<u8> = (0..=255).cycle().take(10_000).collect();
        server.put_bytes("/f", data.clone());
        let fetch = UreqFetch::new();

        let body = fetch.get_from(&server.url("/f"), 4000).unwrap();
        assert_eq!((body.start, body.total), (4000, Some(10_000)));
        assert_eq!(read(body), data[4000..]);
        assert_eq!(server.log()[0].range.as_deref(), Some("bytes=4000-"));

        // Already complete: nothing left to read.
        let body = fetch.get_from(&server.url("/f"), 10_000).unwrap();
        assert_eq!((body.start, body.total), (10_000, Some(10_000)));
        assert!(read(body).is_empty());
        // Past the end: the cached file is wrong.
        assert!(matches!(
            fetch.get_from(&server.url("/f"), 20_000),
            Err(FetchError::Status { status: 416, .. })
        ));

        // A server that ignores Range sends everything from 0.
        server.put(
            "/g",
            File {
                ignore_range: true,
                ..File::new(data.clone())
            },
        );
        let body = fetch.get_from(&server.url("/g"), 4000).unwrap();
        assert_eq!((body.start, body.total), (0, Some(10_000)));
        assert_eq!(read(body), data);
    }

    #[test]
    fn a_cut_connection_is_an_error_not_a_short_file() {
        let server = Server::start();
        let data = vec![7u8; 50_000];
        server.put(
            "/f",
            File {
                cut_after: Some(1000),
                ..File::new(data.clone())
            },
        );
        let fetch = UreqFetch::new();
        let body = fetch.get_from(&server.url("/f"), 0).unwrap();
        let mut got = Vec::new();
        let mut reader = body.reader;
        assert!(reader.read_to_end(&mut got).is_err());
        assert_eq!(got.len(), 1000);

        // Resume from what arrived.
        let body = fetch.get_from(&server.url("/f"), got.len() as u64).unwrap();
        assert_eq!(body.start, 1000);
        got.extend(read(body));
        assert_eq!(got, data);
    }

    #[test]
    fn redirects_are_followed() {
        let server = Server::start();
        server.put_bytes("/real", "x");
        server.put(
            "/old",
            File {
                redirect: Some("/real".into()),
                ..File::default()
            },
        );
        assert_eq!(UreqFetch::new().get(&server.url("/old"), 10).unwrap(), b"x");
    }
}
