//! Every thread CrossFX names says so: `crossfx-…`, never the program's old name.
//!
//! A thread's name is what `ps -T`, `top -H` and htop list (cut to 15 bytes on Linux), what
//! Process Explorer and a debugger show on Windows, and what a panic on that thread prints; one
//! called `fxsound-…` tells whoever reads them that a program called FxSound is running. The
//! rename missed one, the engine's notification carrier, because `wake::forward` names the thread
//! it starts after its first argument, away from any thread builder.
//!
//! Read off the source of every crate: the string literal each `.name(…)` and each `forward(…)`
//! is handed, directly or as the format string of a `format!`.

use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, recursively.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The text of the string literal `src` starts with, after whitespace and an optional
/// `format!(`, up to its closing quote; `None` when it starts with something else.
fn leading_literal(src: &str) -> Option<&str> {
    let src = src.trim_start();
    let src = src.strip_prefix("format!(").map_or(src, str::trim_start);
    let body = src.strip_prefix('"')?;
    body.find('"').map(|end| &body[..end])
}

/// Every literal name in the workspace's sources, with the file it is written in.
fn names() -> Vec<(PathBuf, String)> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates).expect("the crates are listed") {
        rust_sources(&entry.expect("a crate").path().join("src"), &mut files);
    }
    assert!(files.len() > 100, "only {} source files found", files.len());
    let mut names = Vec::new();
    for file in files {
        let src = std::fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("{}: {err}", file.display()));
        for call in [".name(", "forward("] {
            for (at, _) in src.match_indices(call) {
                if let Some(name) = leading_literal(&src[at + call.len()..]) {
                    names.push((file.clone(), name.to_owned()));
                }
            }
        }
    }
    names
}

#[test]
fn every_thread_crossfx_names_says_crossfx() {
    let names = names();
    let old: Vec<String> = names
        .iter()
        .filter(|(_, name)| name.to_lowercase().contains("fxsound"))
        .map(|(file, name)| format!("{}: {name:?}", file.display()))
        .collect();
    assert!(
        old.is_empty(),
        "named for the program's old name:\n  {}",
        old.join("\n  ")
    );
    // And the scan still reads each way a name is given: a builder's literal, a `format!`, and
    // the carrier `wake::forward` names.
    for expected in ["crossfx-audio", "crossfx-{}", "crossfx-audio-wake"] {
        assert!(
            names.iter().any(|(_, name)| name == expected),
            "the scan no longer finds {expected:?}, so it may miss an old name too: {names:?}"
        );
    }
}

#[test]
fn a_name_is_read_as_written_and_through_format() {
    assert_eq!(
        leading_literal("\"crossfx-ipc\".to_owned())"),
        Some("crossfx-ipc")
    );
    assert_eq!(
        leading_literal("\n    format!(\"crossfx-{}\", key))"),
        Some("crossfx-{}")
    );
    assert_eq!(leading_literal("name.to_owned())"), None);
}
