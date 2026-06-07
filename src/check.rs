use crate::config::CheckConfig;
use crate::listing::{self, DirectoryResponse};
use crate::root::{DiagnosticTarget, RejectReason, Root};
use crate::selector;
use std::io::{self, Write};

pub(crate) fn run(config: &CheckConfig, mut out: impl Write) -> io::Result<bool> {
    let root = Root::open(&config.root)?;
    let mut all_ok = true;

    for selector in &config.selectors {
        match selector::parse_diagnostic(selector) {
            Ok(request) => match root.resolve_diagnostic(&request)? {
                Ok(DiagnosticTarget::File(_fd)) => {
                    writeln!(
                        out,
                        "ok: {}: regular file, public",
                        display_selector(selector)
                    )?;
                }
                Ok(DiagnosticTarget::Dir(fd)) => match listing::diagnose(&root, fd)? {
                    Ok(DirectoryResponse::Index) => {
                        writeln!(
                            out,
                            "ok: {}: directory, public index",
                            display_selector(selector)
                        )?;
                    }
                    Ok(DirectoryResponse::Listing) => {
                        writeln!(
                            out,
                            "ok: {}: directory, public listing",
                            display_selector(selector)
                        )?;
                    }
                    Err(reason) => {
                        all_ok = false;
                        write_reject(&mut out, selector, &reason)?;
                    }
                },
                Err(reason) => {
                    all_ok = false;
                    write_reject(&mut out, selector, &reason)?;
                }
            },
            Err(reason) => {
                all_ok = false;
                writeln!(
                    out,
                    "reject: {}: {}",
                    display_selector(selector),
                    reason.message()
                )?;
            }
        }
    }

    Ok(all_ok)
}

fn write_reject(mut out: impl Write, selector: &str, reason: &RejectReason) -> io::Result<()> {
    writeln!(
        out,
        "reject: {}: {}",
        display_selector(selector),
        reason.message()
    )
}

fn display_selector(selector: &str) -> String {
    selector
        .chars()
        .flat_map(|ch| match ch {
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\0' => "\\0".chars().collect::<Vec<_>>(),
            ch => vec![ch],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempSite;
    use std::fs;

    #[test]
    fn check_outputs_ok_and_reject_lines() {
        let site = TempSite::new();
        site.write("public.txt", b"public\n");
        site.write("listing/page.txt", b"page\n");
        site.write(".secret", b"secret\n");

        let config = CheckConfig {
            root: site.path().to_path_buf(),
            selectors: vec![
                "public.txt".to_string(),
                "listing/".to_string(),
                ".secret".to_string(),
                "missing.txt".to_string(),
            ],
        };
        let mut out = Vec::new();
        let all_ok = run(&config, &mut out).expect("run check");

        assert!(!all_ok);
        assert_eq!(
            String::from_utf8(out).expect("stdout utf8"),
            "\
ok: public.txt: regular file, public
ok: listing/: directory, public listing
reject: .secret: dotfile component
reject: missing.txt: not found
"
        );
    }

    #[cfg(unix)]
    #[test]
    fn check_outputs_policy_reasons() {
        let site = TempSite::new();
        site.write("private.txt", b"private\n");
        site.chmod("private.txt", 0o600);
        site.write("linked.txt", b"linked\n");
        fs::hard_link(
            site.path().join("linked.txt"),
            site.path().join("alias.txt"),
        )
        .expect("create hardlink");
        site.symlink("linked.txt", "link.txt");
        site.write("hidden/inside.txt", b"inside\n");
        site.chmod("hidden", 0o111);

        let config = CheckConfig {
            root: site.path().to_path_buf(),
            selectors: vec![
                "private.txt".to_string(),
                "linked.txt".to_string(),
                "link.txt".to_string(),
                "hidden".to_string(),
            ],
        };
        let mut out = Vec::new();
        let all_ok = run(&config, &mut out).expect("run check");

        assert!(!all_ok);
        assert_eq!(
            String::from_utf8(out).expect("stdout utf8"),
            "\
reject: private.txt: not world-readable
reject: linked.txt: hardlink count 2
reject: link.txt: symlink
reject: hidden: directory is not world-readable
"
        );
    }

    #[test]
    fn check_returns_true_when_every_selector_is_servable() {
        let site = TempSite::new();
        site.write("public.txt", b"public\n");

        let config = CheckConfig {
            root: site.path().to_path_buf(),
            selectors: vec!["public.txt".to_string()],
        };
        let mut out = Vec::new();
        let all_ok = run(&config, &mut out).expect("run check");

        assert!(all_ok);
        assert_eq!(
            String::from_utf8(out).expect("stdout utf8"),
            "ok: public.txt: regular file, public\n"
        );
    }
}
