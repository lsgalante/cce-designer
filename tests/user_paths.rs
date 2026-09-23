//! The crate's own source, checked for paths that belong to the user.
//!
//! An integration test on purpose, for the reason `doc_claims.rs` gives about
//! its own scans: anything under `src/` is counted by them, and a scanner
//! living there is the first thing it finds. From out here the needles can be
//! written plainly, because this file is not in what it reads.
//!
//! Deliberately MIRRORED per crate rather than shared from a helper: each is
//! its own git repository that must build standalone, and this needs nothing
//! but std. Same call `doc_claims.rs` makes.

pub mod user_paths {
    use std::path::{Path, PathBuf};

    /// Every `.rs` file under `src/`, as (path, contents).
    fn sources(root: &Path) -> Vec<(PathBuf, String)> {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        out.push((p, s));
                    }
                }
            }
        }
        let mut v = Vec::new();
        walk(&root.join("src"), &mut v);
        v
    }

    /// Prose says what the code used to do, so only code counts.
    fn is_comment(line: &str) -> bool {
        let t = line.trim_start();
        t.starts_with("//") || t.starts_with("*")
    }

    /// `cargo test` must not touch anything the user owns. Two shapes of that
    /// shipped from this crate, and this is the scan that would have caught
    /// either the day it was written.
    ///
    /// **A config path built by hand.** `DesignSettings::file_path` spelled
    /// out `$HOME/.config/cce/cce-designer/state.kdl`. `State::new` loads the
    /// bundled project, whose meta subnets overwrite the live viewport flags,
    /// so every test that reached `save_settings` wrote the bundled project's
    /// show_grid / show_cube / show_origin over the user's own. The suite
    /// reset three of their toggles on every run and stayed green. The path
    /// goes through `cce_ui::config::cce_config_dir()` now, which is what the
    /// `cfg(test)` redirect keys off — so hand-assembling one out of
    /// `.config` is precisely the regression to refuse, and going through the
    /// toolkit is the only way in.
    ///
    /// **A scratch path shared with the rest of the machine.** Two tests kept
    /// their files at fixed `/tmp` names. /tmp is one namespace shared by
    /// every user — the point `../cce-compositor/WORKSPACE.md` makes about
    /// `cce_runtime_dir` — so a fixed name belongs to whoever ran first and
    /// the sticky bit denies it to everyone else; nearer to hand, two
    /// checkouts running their suites at once shared one directory. Scoping
    /// by pid is the crate's own convention, followed at every other site.
    ///
    /// The rule is on `temp_dir()` rather than on tests alone because a fixed
    /// /tmp path is no better in shipped code; if one ever needs an unscoped
    /// name it should earn a line here saying why.
    #[test]
    fn no_source_builds_a_path_the_user_owns() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let files = sources(root);
        assert!(!files.is_empty(), "found no sources under src/ — the walk is broken");

        let mut bad: Vec<String> = Vec::new();
        for (path, src) in &files {
            let name = path.strip_prefix(root).unwrap_or(path).display();
            let lines: Vec<&str> = src.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if is_comment(line) {
                    continue;
                }
                if line.contains("\".config\"") || line.contains("/.config") {
                    bad.push(format!(
                        "{name}:{}: builds a config path by hand — go through \
                         cce_ui::config::cce_config_dir(), which is what the \
                         cfg(test) redirect keys off:\n      {}",
                        i + 1,
                        line.trim()
                    ));
                }
                // The call may wrap, so the scoping can be a line or two down.
                if line.contains("temp_dir()") {
                    let window = lines[i..lines.len().min(i + 3)].join(" ");
                    if !window.contains("process::id()") {
                        bad.push(format!(
                            "{name}:{}: a scratch path shared with every other \
                             user and every concurrent run — scope it with \
                             std::process::id():\n      {}",
                            i + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "source builds paths the user owns:\n  {}", bad.join("\n  "));
    }
}
