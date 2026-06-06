//! Reference-only characterization tests for the Go `nexd` server.
//!
//! These pin the observable behavior of the local reference implementation so
//! `buffetcar` can deliberately match protocol-compatible cases and invert the
//! `legacy_behavior` cases that the design exists to fix. Run with:
//! `cargo test --features nexd-contract`.

mod common;

use common::{request, unique_name, Nexd, TempSite};
use std::fs;

#[test]
fn nexd_serves_root_index_files_directory_indexes_and_not_found() {
    let site = TempSite::new();
    site.write("index", "root index\n");
    site.write("plain.txt", "plain file\n");
    site.write("docs/index", "docs index\n");

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
    site.write("plain.txt", "plain file\n");
    site.write("docs/index", "docs index\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request("/plain.txt"), b"plain file\n");
    assert_eq!(request("plain.txt/"), b"plain file\n");
    assert_eq!(request("/docs/"), b"docs index\n");
}

#[test]
fn nexd_preserves_binary_file_bytes() {
    let site = TempSite::new();
    let bytes = [0, 1, 2, b'\n', 0xff, b'n', b'e', b'x'];
    fs::write(site.path().join("blob.bin"), bytes).expect("write binary fixture");

    let _server = Nexd::start(site.path());

    assert_eq!(request("blob.bin"), bytes);
}

#[test]
fn nexd_generates_ascending_directory_listings_and_hides_dotfiles() {
    let site = TempSite::new();
    site.dir("listing/subdir");
    site.write("listing/apple.txt", "apple\n");
    site.write("listing/banana.txt", "banana\n");
    site.write("listing/.hidden", "hidden\n");

    let _server = Nexd::start(site.path());

    assert_eq!(
        request("listing"),
        b"=> apple.txt\n=> banana.txt\n=> subdir/\n"
    );
}

#[cfg(unix)]
#[test]
fn nexd_omits_entries_without_world_read_permission_from_listings() {
    use std::os::unix::fs::PermissionsExt;

    let site = TempSite::new();
    site.write("listing/private.txt", "private\n");
    site.write("listing/public.txt", "public\n");
    fs::set_permissions(
        site.path().join("listing/private.txt"),
        fs::Permissions::from_mode(0o600),
    )
    .expect("make fixture private");

    let _server = Nexd::start(site.path());

    assert_eq!(request("listing"), b"=> public.txt\n");
}

#[test]
fn nexd_reverses_directory_listings_when_desc_marker_exists() {
    let site = TempSite::new();
    site.write("listing/apple.txt", "apple\n");
    site.write("listing/banana.txt", "banana\n");
    site.write("listing/cherry.txt", "cherry\n");
    site.write("listing/.desc", "");

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
    site.write("a/c.txt", "inside root\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request("a/b/../c.txt"), b"document not found");
}

#[test]
fn nexd_legacy_behavior_serves_direct_dotfile_requests() {
    let site = TempSite::new();
    site.write(".secret", "secret\n");

    let _server = Nexd::start(site.path());

    assert_eq!(request(".secret"), b"secret\n");
}

#[test]
fn nexd_rejects_parent_traversal_selectors() {
    let site = TempSite::new();
    let outside_name = unique_name("buffetcar-outside", ".txt");
    let outside = site
        .path()
        .parent()
        .expect("site has parent")
        .join(&outside_name);
    fs::write(&outside, "outside root\n").expect("write outside fixture");

    let _server = Nexd::start(site.path());
    let response = request(&format!("../{outside_name}"));

    fs::remove_file(&outside).expect("remove outside fixture");
    assert_eq!(response, b"document not found");
}

#[cfg(unix)]
#[test]
fn nexd_legacy_behavior_follows_symlinks_outside_the_root() {
    use std::os::unix::fs::symlink;

    let site = TempSite::new();
    let outside_name = unique_name("buffetcar-symlink-target", ".txt");
    let outside = site
        .path()
        .parent()
        .expect("site has parent")
        .join(outside_name);
    fs::write(&outside, "symlink target\n").expect("write symlink target");
    symlink(&outside, site.path().join("leak.txt")).expect("create symlink fixture");

    let _server = Nexd::start(site.path());
    let response = request("leak.txt");

    fs::remove_file(&outside).expect("remove symlink target");
    assert_eq!(response, b"symlink target\n");
}
