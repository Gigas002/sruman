//! The tar extractor on arbitrary archives: an error or a tree inside the
//! destination, never a panic or a write outside it.
#![no_main]

use libfuzzer_sys::fuzz_target;
use libsruman::{
    install::extract::{Limits, extract_tar},
    progress::CancelFlag,
};

fuzz_target!(|data: &[u8]| {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    let limits = Limits {
        bytes: 16 << 20,
        entries: 10_000,
    };
    let _ = extract_tar(data, &dest, limits, &CancelFlag::new(), &mut || {});
    let outside: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(outside, ["dest"], "wrote outside the destination");
});
