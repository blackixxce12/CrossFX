//! The maintainer scripts under `scripts/` say what they do and how to call them in the comment
//! each one opens with, and `--help` prints that comment — all of it. `reference-checkout.sh`
//! printed a fixed range of its lines instead, and when the crate rename made the comment three
//! lines longer, its `--help` stopped in the middle of a sentence and left out the usage line.

// The scripts are bash, run as a maintainer runs them.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};

use crossfx_core::test_support::command;

/// The repository this test binary was built from.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root exists")
}

/// Every script under `scripts/` with a `-h | --help` arm, as (its path from the repository, its
/// text).
fn scripts_with_help() -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut pending = vec![repo().join("scripts")];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("the directory is listed") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "sh") {
                continue;
            }
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()));
            let answers_help = text.lines().any(|line| {
                let arm = line.trim_start();
                arm.starts_with("-h | --help)") || arm.starts_with("-h|--help)")
            });
            if answers_help {
                let relative = path
                    .strip_prefix(repo())
                    .expect("under the repository")
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                found.push((relative, text));
            }
        }
    }
    found.sort();
    found
}

/// The comment a script opens with, as `--help` is to print it: every line after the `#!` line up
/// to the first that is not a comment, without its `#` and the one space after it.
fn opening_comment(text: &str) -> String {
    let mut comment = String::new();
    for line in text
        .lines()
        .skip(1)
        .map_while(|line| line.strip_prefix('#'))
    {
        comment.push_str(line.strip_prefix(' ').unwrap_or(line));
        comment.push('\n');
    }
    comment
}

#[test]
fn every_scripts_help_prints_the_whole_comment_it_opens_with() {
    let scripts = scripts_with_help();
    let names: Vec<&str> = scripts.iter().map(|(name, _)| name.as_str()).collect();
    for script in [
        "scripts/reference-checkout.sh",
        "scripts/windows-parity-bitexact.sh",
    ] {
        assert!(names.contains(&script), "{script} is not among {names:?}");
    }
    for (script, text) in &scripts {
        let run = command("bash")
            .arg(script)
            .arg("--help")
            .current_dir(repo())
            .output()
            .expect("bash runs");
        assert!(
            run.status.success(),
            "{script} --help failed ({}): {}",
            run.status,
            String::from_utf8_lossy(&run.stderr)
        );
        let printed = String::from_utf8(run.stdout).expect("the help is UTF-8");
        assert_eq!(
            printed,
            opening_comment(text),
            "{script} --help is not the comment it opens with"
        );
    }
}
