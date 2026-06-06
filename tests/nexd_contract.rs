//! Reference-only characterization tests for the Go `nexd` server.
//!
//! These pin observable behavior of the local reference implementation so
//! buffetcar can deliberately preserve protocol-compatible cases and invert
//! unsafe legacy cases. Run with:
//! `cargo test --features nexd-contract --test nexd_contract`.

mod common;

use common::{request, Nexd, OutsideFile, TempSite};

#[test]
fn nexd_serves_root_index_files_directory_indexes_and_not_found() {
    let site = TempSite::new();
    site.write("index", b"root index\n");
    site.write("plain.txt", b"plain file\n");
    site.write("docs/index", b"docs index\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request(""), b"root index\n");
    assert_eq!(request("/"), b"root index\n");
    assert_eq!(request("plain.txt"), b"plain file\n");
    assert_eq!(request("docs"), b"docs index\n");
    assert_eq!(request("docs/"), b"docs index\n");
    assert_eq!(request("missing.txt"), b"document not found");
}

#[test]
fn nexd_trims_leading_and_trailing_slashes_from_selectors() {
    let site = TempSite::new();
    site.write("plain.txt", b"plain file\n");
    site.write("docs/index", b"docs index\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request("/plain.txt"), b"plain file\n");
    assert_eq!(request("plain.txt/"), b"plain file\n");
    assert_eq!(request("/docs/"), b"docs index\n");
}

#[test]
fn nexd_preserves_binary_file_bytes() {
    let site = TempSite::new();
    let bytes = [0, 1, 2, b'\n', 0xff, b'n', b'e', b'x'];
    site.write("blob.bin", bytes);

    let _server = Nexd::start(site.path());

    assert_eq!(request("blob.bin"), bytes);
}

#[test]
fn nexd_generates_ascending_directory_listings_and_hides_dotfiles() {
    let site = TempSite::new();
    site.dir("listing/subdir");
    site.write("listing/apple.txt", b"apple\n");
    site.write("listing/banana.txt", b"banana\n");
    site.write("listing/.hidden", b"hidden\n");

    let _server = Nexd::start(site.path());

    assert_eq!(
        request("listing"),
        b"=> apple.txt\n=> banana.txt\n=> subdir/\n"
    );
}

#[cfg(unix)]
#[test]
fn nexd_omits_entries_without_world_read_permission_from_listings() {
    let site = TempSite::new();
    site.write_private("listing/private.txt", b"private\n");
    site.write("listing/public.txt", b"public\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request("listing"), b"=> public.txt\n");
}

#[test]
fn nexd_reverses_directory_listings_when_desc_marker_exists() {
    let site = TempSite::new();
    site.write("listing/apple.txt", b"apple\n");
    site.write("listing/banana.txt", b"banana\n");
    site.write("listing/cherry.txt", b"cherry\n");
    site.write("listing/.desc", b"");

    let _server = Nexd::start(site.path());

    assert_eq!(
        request("listing"),
        b"=> cherry.txt\n=> banana.txt\n=> apple.txt\n"
    );
}

#[test]
fn nexd_rejects_selectors_containing_parent_components_even_when_balanced() {
    let site = TempSite::new();
    site.dir("a/b");
    site.write("a/c.txt", b"inside root\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request("a/b/../c.txt"), b"document not found");
}

#[test]
fn nexd_rejects_parent_traversal_selectors() {
    let site = TempSite::new();
    let outside = OutsideFile::new(&site, "buffetcar-outside", b"outside root\n");

    let _server = Nexd::start(site.path());
    let response = request(&format!("../{}", outside.name()));

    assert_eq!(response, b"document not found");
}

#[test]
fn nexd_legacy_behavior_serves_direct_dotfile_requests() {
    let site = TempSite::new();
    site.write(".secret", b"secret\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request(".secret"), b"secret\n");
}

#[cfg(unix)]
#[test]
fn nexd_legacy_behavior_follows_symlinks_outside_the_root() {
    use std::os::unix::fs::symlink;

    let site = TempSite::new();
    let outside = OutsideFile::new(&site, "buffetcar-symlink-target", b"symlink target\n");
    symlink(outside.path(), site.path().join("leak.txt")).expect("create symlink fixture");

    let _server = Nexd::start(site.path());

    assert_eq!(request("leak.txt"), b"symlink target\n");
}

#[cfg(unix)]
#[test]
fn nexd_legacy_behavior_follows_in_root_symlink_to_dotfile() {
    use std::os::unix::fs::symlink;

    let site = TempSite::new();
    site.write(".secret", b"secret\n");
    symlink(".secret", site.path().join("public")).expect("create symlink fixture");

    let _server = Nexd::start(site.path());

    assert_eq!(request("public"), b"secret\n");
}

#[cfg(unix)]
#[test]
fn nexd_legacy_behavior_serves_symlinked_index() {
    use std::os::unix::fs::symlink;

    let site = TempSite::new();
    site.write("docs/.secret", b"secret index\n");
    site.write("docs/page.txt", b"page\n");
    symlink(".secret", site.path().join("docs/index")).expect("create symlink fixture");

    let _server = Nexd::start(site.path());

    assert_eq!(request("docs"), b"secret index\n");
}

#[cfg(unix)]
#[test]
fn nexd_legacy_behavior_serves_private_file_when_daemon_user_can_read_it() {
    let site = TempSite::new();
    site.write_private("private.txt", b"private\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request("private.txt"), b"private\n");
}

#[cfg(unix)]
#[test]
fn nexd_legacy_behavior_serves_hardlink_to_file_outside_root() {
    let site = TempSite::new();
    let outside = OutsideFile::new(&site, "buffetcar-hardlink-target", b"hardlink target\n");

    let link = site.path().join("published-hardlink.txt");
    if let Err(err) = std::fs::hard_link(outside.path(), &link) {
        eprintln!("skipping hardlink characterization: hard_link unsupported here: {err}");
        return;
    }

    let _server = Nexd::start(site.path());

    assert_eq!(request("published-hardlink.txt"), b"hardlink target\n");
}
