//! Every parser of downloaded or on-disk text: `SHA256SUMS`, os-release and
//! pointer files, autoindex pages, `manifest.dpkg`, mtree, `ldconfig -p`,
//! `Content-Range`, build ids and runtime refs, and the TOML files.
#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;
use libsruman::{
    manifest::Manifest,
    mtree,
    overlay::HostLibs,
    version::{BuildId, Channel, RuntimeRef},
};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = text.parse::<BuildId>();
    let _ = text.parse::<RuntimeRef>();
    let _ = text.parse::<Channel>();
    let _ = Manifest::parse(text);
    let _ = mtree::parse(text, Path::new("mtree.txt.gz"));
    let _ = HostLibs::parse(text);
    let _ = libsruman::sync::parse_sha256sums(text);
    let _ = libsruman::sync::parse_os_release(text);
    let _ = libsruman::sync::parse_autoindex(text, None);
    let _ = libsruman::fetch::parse_content_range(text);
    let _ = toml::from_str::<libsruman::model::Game>(text);
});
