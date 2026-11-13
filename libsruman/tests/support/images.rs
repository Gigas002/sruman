//! A fake `repo.steampowered.com` `images/` tree on the test HTTP server.

#![allow(dead_code)]

use std::io::Write;

use sha2::{Digest, Sha256};

use super::http::{File, Server};

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A container archive shaped like Valve's: `<top>/run`, `VERSIONS.txt`,
/// `<suite>_platform_<build>/files/manifest.dpkg`, and a relative symlink.
pub fn container_tar_xz(top: &str, suite: &str, build: &str, manifest: &str) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut add = |path: &str, data: &[u8], mode: u32| {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(mode);
        header.set_mtime(1_700_000_000);
        header.set_cksum();
        builder.append_data(&mut header, path, data).unwrap();
    };
    add(&format!("{top}/run"), b"#!/bin/sh\nexec \"$@\"\n", 0o755);
    add(
        &format!("{top}/VERSIONS.txt"),
        format!("#Name\tVersion\ndepot\t{build}\n").as_bytes(),
        0o644,
    );
    add(
        &format!("{top}/{suite}_platform_{build}/files/manifest.dpkg"),
        manifest.as_bytes(),
        0o644,
    );
    add(
        &format!("{top}/pressure-vessel/libexec/tool"),
        b"tool",
        0o755,
    );
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_cksum();
    builder
        .append_link(
            &mut header,
            format!("{top}/pressure-vessel/bin/tool"),
            "../libexec/tool",
        )
        .unwrap();
    let tar = builder.into_inner().unwrap();
    let mut xz =
        lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1)).unwrap();
    xz.write_all(&tar).unwrap();
    xz.finish().unwrap()
}

/// One runtime's directory on the server (`/steamrt3/images/...`).
pub struct Suite<'a> {
    pub server: &'a Server,
    pub dir: &'a str,
    pub suite: &'a str,
    pub archive: &'a str,
}

impl Suite<'_> {
    fn images(&self) -> String {
        format!("/{}/images", self.dir)
    }

    /// Publishes a build; returns the archive bytes.
    pub fn publish(&self, build: &str, manifest: &str) -> Vec<u8> {
        let top = self.archive.trim_end_matches(".tar.xz");
        let data = container_tar_xz(top, self.suite, build, manifest);
        let base = format!("{}/{build}", self.images());
        let platform = format!(
            "com.valvesoftware.SteamRuntime.Platform-amd64,i386-{}",
            self.suite
        );
        self.server.put_bytes(
            &format!("{base}/SHA256SUMS"),
            format!(
                "{} *{}\n{} *{platform}.manifest.dpkg\n{} *{platform}.os-release.txt\n",
                sha256_hex(&data),
                self.archive,
                sha256_hex(manifest.as_bytes()),
                sha256_hex(b"x"),
            ),
        );
        self.server
            .put_bytes(&format!("{base}/{}", self.archive), data.clone());
        self.server
            .put_bytes(&format!("{base}/{platform}.manifest.dpkg"), manifest);
        self.server.put_bytes(
            &format!("{base}/{platform}.os-release.txt"),
            format!(
                "PRETTY_NAME=\"Steam Runtime ({})\"\nBUILD_ID=\"{build}\"\n",
                self.suite
            ),
        );
        data
    }

    /// Points `channel_file` (`latest-public-stable`) at `build` and bumps
    /// the timestamp.
    pub fn point(&self, channel_file: &str, build: &str, stamp: &str) {
        self.server.put_bytes(
            &format!("{}/{channel_file}.txt", self.images()),
            format!("{build}\n"),
        );
        self.server
            .put_bytes(&format!("{}/update-timestamp.txt", self.images()), stamp);
    }

    /// Replaces a published archive (for truncation and corruption tests).
    pub fn put_archive(&self, build: &str, file: File) {
        self.server
            .put(&format!("{}/{build}/{}", self.images(), self.archive), file);
    }

    pub fn archive_path(&self, build: &str) -> String {
        format!("{}/{build}/{}", self.images(), self.archive)
    }
}
