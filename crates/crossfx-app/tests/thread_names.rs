//! No thread CrossFX names carries the program's old name.
//!
//! A thread's name is what `ps -T`, `top -H` and htop list (cut to 15 bytes on Linux), what
//! Process Explorer and a debugger show on Windows, and what a panic on that thread prints; one
//! called `fxsound-…` tells whoever reads them that a program called FxSound is running. The
//! rename missed one, the engine's notification carrier, because `wake::forward` names the thread
//! it starts after its first argument, away from any thread builder.
//!
//! Read off the source of every crate, a thread builder at a time. Each `thread::Builder::new()`,
//! or `Builder::new()` where `std::thread`'s `Builder` is imported, names its thread in its own
//! chain, `.name(…)` before `.spawn(…)`, with a name the scan reads:
//! a string literal, a `const` or `static` of one, or `format!` of them — as written or through
//! `.into()`, `.to_owned()`, `.to_string()` or `String::from` — or a parameter of the function the
//! builder is in, which is then read at every call of that function the same way. Whatever the
//! scan cannot read fails the test rather than passing unread: a builder that names nothing in its
//! chain, a name from anything else, a carrier nothing calls. `.name(…)` anywhere but on a thread
//! builder — a D-Bus connection's, a device's, a preset's — is not a thread's name and is not read,
//! and neither is anything in a comment.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------------------------

/// What one token of Rust source is. Whitespace and comments are not tokens.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Ident(String),
    /// A string literal's text, its escapes read.
    Str(String),
    Punct(char),
    /// A number, a character or a lifetime: nothing a name is read from.
    Other,
}

#[derive(Debug, Clone)]
struct Token {
    kind: Kind,
    line: usize,
}

/// The text of a quoted literal whose body starts at `chars[at]`, its escapes read, and the index
/// after its closing quote.
fn quoted(chars: &[char], mut at: usize) -> (String, usize) {
    let mut text = String::new();
    while let Some(&c) = chars.get(at) {
        at += 1;
        match c {
            '"' => break,
            '\\' => {
                let Some(&escaped) = chars.get(at) else { break };
                at += 1;
                match escaped {
                    'n' => text.push('\n'),
                    't' => text.push('\t'),
                    '\n' => {
                        while chars.get(at).is_some_and(|c| c.is_whitespace()) {
                            at += 1;
                        }
                    }
                    other => text.push(other),
                }
            }
            _ => text.push(c),
        }
    }
    (text, at)
}

/// The tokens of `src`, each with its line.
fn tokens(src: &str) -> Vec<Token> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut line = 1;
    let mut at = 0;
    let lines_in = |range: Range<usize>| chars[range].iter().filter(|&&c| c == '\n').count();
    while let Some(&c) = chars.get(at) {
        let next = chars.get(at + 1).copied();
        let start = at;
        let raw = if matches!(c, 'r' | 'b' | 'c') {
            raw_string(&chars, at)
        } else {
            None
        };
        if c.is_whitespace() {
            at += 1;
        } else if c == '/' && next == Some('/') {
            while chars.get(at).is_some_and(|&c| c != '\n') {
                at += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while at < chars.len() {
                if chars[at] == '/' && chars.get(at + 1) == Some(&'*') {
                    depth += 1;
                    at += 2;
                } else if chars[at] == '*' && chars.get(at + 1) == Some(&'/') {
                    depth -= 1;
                    at += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    at += 1;
                }
            }
        } else if c == '"' {
            let (text, end) = quoted(&chars, at + 1);
            out.push(Token {
                kind: Kind::Str(text),
                line,
            });
            at = end;
        } else if let Some((text, end)) = raw {
            out.push(Token {
                kind: Kind::Str(text),
                line,
            });
            at = end;
        } else if c.is_alphabetic() || c == '_' {
            while chars
                .get(at)
                .is_some_and(|&c| c.is_alphanumeric() || c == '_')
            {
                at += 1;
            }
            let word: String = chars[start..at].iter().collect();
            // A raw identifier, `r#type`, is the identifier.
            if word == "r" && chars.get(at) == Some(&'#') {
                at += 1;
                continue;
            }
            out.push(Token {
                kind: Kind::Ident(word),
                line,
            });
        } else if c.is_ascii_digit() {
            while chars
                .get(at)
                .is_some_and(|&c| c.is_alphanumeric() || c == '_')
                || (chars.get(at) == Some(&'.')
                    && chars.get(at + 1).is_some_and(char::is_ascii_digit))
            {
                at += 1;
            }
            out.push(Token {
                kind: Kind::Other,
                line,
            });
        } else if c == '\'' {
            // A character, `'x'`, `'\''` or `'\u{2019}'`, or a lifetime, `'a`.
            if next == Some('\\') {
                // The quote, the backslash and the character it escapes, then up to the quote.
                at += 3;
                while chars.get(at).is_some_and(|&c| c != '\'') {
                    at += 1;
                }
                at += 1;
            } else if chars.get(at + 2) == Some(&'\'') {
                at += 3;
            } else {
                at += 1;
                while chars
                    .get(at)
                    .is_some_and(|&c| c.is_alphanumeric() || c == '_')
                {
                    at += 1;
                }
            }
            out.push(Token {
                kind: Kind::Other,
                line,
            });
        } else {
            out.push(Token {
                kind: Kind::Punct(c),
                line,
            });
            at += 1;
        }
        line += lines_in(start..at.min(chars.len()));
    }
    out
}

/// A raw string, byte string or C string starting at `chars[at]` — `r"…"`, `r#"…"#`, `b"…"`,
/// `br"…"`, `c"…"` — as its text and the index after it; `None` when none starts there.
fn raw_string(chars: &[char], mut at: usize) -> Option<(String, usize)> {
    if matches!(chars.get(at), Some('b' | 'c')) {
        at += 1;
        if chars.get(at) == Some(&'"') {
            return Some(quoted(chars, at + 1));
        }
    }
    if chars.get(at) != Some(&'r') {
        return None;
    }
    at += 1;
    let mut hashes = 0;
    while chars.get(at) == Some(&'#') {
        hashes += 1;
        at += 1;
    }
    if chars.get(at) != Some(&'"') {
        return None;
    }
    at += 1;
    let start = at;
    while at < chars.len() {
        if chars[at] == '"' && (1..=hashes).all(|n| chars.get(at + n) == Some(&'#')) {
            let text = chars[start..at].iter().collect();
            return Some((text, at + 1 + hashes));
        }
        at += 1;
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Reading a file's tokens
// ---------------------------------------------------------------------------------------------

/// A source file: its path under `crates/`, and its tokens.
struct Source {
    path: String,
    tokens: Vec<Token>,
}

impl Source {
    fn new(path: &str, src: &str) -> Self {
        Self {
            path: path.to_owned(),
            tokens: tokens(src),
        }
    }

    fn kind(&self, at: usize) -> Option<&Kind> {
        self.tokens.get(at).map(|token| &token.kind)
    }

    fn is(&self, at: usize, punct: char) -> bool {
        self.kind(at) == Some(&Kind::Punct(punct))
    }

    fn ident(&self, at: usize) -> Option<&str> {
        match self.kind(at) {
            Some(Kind::Ident(word)) => Some(word),
            _ => None,
        }
    }

    /// Whether the tokens from `at` on are `a :: b :: …`, the words in `path`.
    fn path_at(&self, at: usize, path: &[&str]) -> bool {
        path.iter().enumerate().all(|(n, &word)| {
            let at = at + 3 * n;
            self.ident(at) == Some(word)
                && (n + 1 == path.len() || (self.is(at + 1, ':') && self.is(at + 2, ':')))
        })
    }

    /// Where the file and line of token `at` are, for a message.
    fn place(&self, at: usize) -> String {
        let line = self.tokens.get(at).map_or(0, |token| token.line);
        format!("{}:{line}", self.path)
    }

    /// The index of the bracket that closes the one at `open`.
    fn closing(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        for at in open..self.tokens.len() {
            match self.kind(at) {
                Some(Kind::Punct('(' | '[' | '{')) => depth += 1,
                Some(Kind::Punct(')' | ']' | '}')) => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(at);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The index after the `>` that closes the `<` at `open`; the `>` of a `->` closes nothing.
    fn after_generics(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut at = open;
        while at < self.tokens.len() {
            if self.is(at, '-') && self.is(at + 1, '>') {
                at += 2;
                continue;
            }
            if self.is(at, '<') {
                depth += 1;
            } else if self.is(at, '>') {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(at + 1);
                }
            }
            at += 1;
        }
        None
    }

    /// `range` cut at its top-level commas.
    fn split(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let mut parts = Vec::new();
        let mut depth = 0usize;
        let mut start = range.start;
        for at in range.clone() {
            match self.kind(at) {
                Some(Kind::Punct('(' | '[' | '{')) => depth += 1,
                Some(Kind::Punct(')' | ']' | '}')) => depth = depth.saturating_sub(1),
                Some(Kind::Punct(',')) if depth == 0 => {
                    parts.push(start..at);
                    start = at + 1;
                }
                _ => {}
            }
        }
        if start < range.end {
            parts.push(start..range.end);
        }
        parts
    }
}

/// A function the scan may have to follow a name into: its name, its parameters other than
/// `self`, whether it takes `self`, and the tokens of its body.
struct Function {
    name: String,
    params: Vec<String>,
    takes_self: bool,
    body: Range<usize>,
}

/// Every function of `source` that has a body.
fn functions(source: &Source) -> Vec<Function> {
    let mut found = Vec::new();
    for at in 0..source.tokens.len() {
        if source.ident(at) != Some("fn") {
            continue;
        }
        let Some(name) = source.ident(at + 1) else {
            continue;
        };
        let mut open = at + 2;
        if source.is(open, '<') {
            let Some(after) = source.after_generics(open) else {
                continue;
            };
            open = after;
        }
        if !source.is(open, '(') {
            continue;
        }
        let Some(close) = source.closing(open) else {
            continue;
        };
        let mut params = Vec::new();
        let mut takes_self = false;
        for param in source.split(open + 1..close) {
            let colon = param
                .clone()
                .find(|&at| {
                    source.is(at, ':') && !source.is(at + 1, ':') && !source.is(at - 1, ':')
                })
                .unwrap_or(param.end);
            let pattern = param.start..colon;
            if pattern.clone().any(|at| source.ident(at) == Some("self")) {
                takes_self = true;
            } else if let Some(word) = pattern.rev().find_map(|at| source.ident(at)) {
                params.push(word.to_owned());
            }
        }
        // The body: the first `{` outside the return type's and the bounds' brackets, unless a
        // `;` ends a declaration first.
        let mut depth = 0usize;
        let mut body = None;
        for at in close + 1..source.tokens.len() {
            match source.kind(at) {
                Some(Kind::Punct('(' | '[')) => depth += 1,
                Some(Kind::Punct(')' | ']')) => depth = depth.saturating_sub(1),
                Some(Kind::Punct('{')) if depth == 0 => {
                    body = source.closing(at).map(|end| at..end);
                    break;
                }
                Some(Kind::Punct(';')) if depth == 0 => break,
                _ => {}
            }
        }
        if let Some(body) = body {
            found.push(Function {
                name: name.to_owned(),
                params,
                takes_self,
                body,
            });
        }
    }
    found
}

/// The module a file is, as a path names it: its stem, its folder's name for a `mod.rs`, `crate`
/// for a crate root.
fn module_of(path: &str) -> String {
    let path = Path::new(path);
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    match stem.as_str() {
        "mod" => path
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        "lib" | "main" => "crate".to_owned(),
        _ => stem,
    }
}

// ---------------------------------------------------------------------------------------------
// The scan
// ---------------------------------------------------------------------------------------------

/// A thread's name as the scan read it — with `{}` where `format!` puts something it does not
/// read, after the text it does — and where it is written.
#[derive(Debug, Clone, PartialEq)]
struct Named {
    name: String,
    at: String,
}

/// What the scan found: the names, and what it could not read.
#[derive(Debug, Default)]
struct Scan {
    names: Vec<Named>,
    unreadable: Vec<String>,
    /// How many thread builders it read.
    builders: usize,
}

/// What an expression handed to a thread's name is.
enum Value {
    /// The texts it can be: one, or one per definition of a constant defined more than once.
    Texts(Vec<String>),
    /// The parameter `index` of the function `function` of file `file`.
    Parameter {
        file: usize,
        function: usize,
        index: usize,
    },
}

/// How many carriers deep a name is followed before the scan gives up on it.
const MAX_DEPTH: usize = 4;

struct Scanner<'a> {
    sources: &'a [Source],
    functions: Vec<Vec<Function>>,
    /// Every `const` and `static` of a string literal, by name.
    constants: BTreeMap<String, Vec<String>>,
    scan: Scan,
}

impl<'a> Scanner<'a> {
    fn new(sources: &'a [Source]) -> Self {
        let mut constants: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for source in sources {
            for at in 0..source.tokens.len() {
                if !matches!(source.ident(at), Some("const" | "static")) {
                    continue;
                }
                let at = if source.ident(at + 1) == Some("mut") {
                    at + 1
                } else {
                    at
                };
                let Some(name) = source.ident(at + 1) else {
                    continue;
                };
                if !source.is(at + 2, ':') {
                    continue;
                }
                let Some(equals) = (at + 3..source.tokens.len())
                    .take_while(|&at| !source.is(at, ';'))
                    .find(|&at| source.is(at, '='))
                else {
                    continue;
                };
                if let Some(Kind::Str(text)) = source.kind(equals + 1)
                    && source.is(equals + 2, ';')
                {
                    constants
                        .entry(name.to_owned())
                        .or_default()
                        .push(text.clone());
                }
            }
        }
        Self {
            sources,
            functions: sources.iter().map(functions).collect(),
            constants,
            scan: Scan::default(),
        }
    }

    fn run(mut self) -> Scan {
        let sources = self.sources;
        for (file, source) in sources.iter().enumerate() {
            for at in 0..source.tokens.len() {
                let Some(chain) = self.thread_builder(source, at) else {
                    continue;
                };
                self.scan.builders += 1;
                match self.named(source, chain) {
                    Some(name) => self.read(file, name, 0),
                    None => self.scan.unreadable.push(format!(
                        "{}: a thread builder that does not name its thread in its own chain",
                        source.place(at)
                    )),
                }
            }
        }
        self.scan
    }

    /// Whether a thread builder, `thread::Builder::new()` or a `Builder::new()` the file imports
    /// from `std::thread`, starts at `at`: the index after its `)` if so.
    fn thread_builder(&self, source: &Source, at: usize) -> Option<usize> {
        let qualified = source.path_at(at, &["thread", "Builder", "new"]);
        let imported = source.path_at(at, &["Builder", "new"])
            && !(at >= 1 && source.is(at - 1, ':'))
            && imports(source, "thread", "Builder");
        let new = if qualified { at + 6 } else { at + 3 };
        ((qualified || imported) && source.is(new + 1, '(') && source.is(new + 2, ')'))
            .then_some(new + 3)
    }

    /// The argument `.name(…)` is handed in the method chain from `at` on, before `.spawn(…)`.
    fn named(&self, source: &Source, mut at: usize) -> Option<Range<usize>> {
        loop {
            if !source.is(at, '.') {
                return None;
            }
            let method = source.ident(at + 1)?;
            if !source.is(at + 2, '(') {
                return None;
            }
            let close = source.closing(at + 2)?;
            match method {
                "name" => return Some(at + 3..close),
                "spawn" | "spawn_scoped" | "spawn_unchecked" => return None,
                _ => at = close + 1,
            }
        }
    }

    /// Read the name `expr`, written in file `file`, into the scan: a text, or the calls of the
    /// function whose parameter it is.
    fn read(&mut self, file: usize, expr: Range<usize>, depth: usize) {
        let at = self.sources[file].place(expr.start);
        match self.value(file, expr) {
            Ok(Value::Texts(texts)) => {
                for name in texts {
                    self.scan.names.push(Named {
                        name,
                        at: at.clone(),
                    });
                }
            }
            Ok(Value::Parameter {
                file: defined,
                function,
                index,
            }) if depth < MAX_DEPTH => {
                let calls = self.calls(defined, function, index);
                if calls.is_empty() {
                    let carrier = &self.functions[defined][function].name;
                    self.scan.unreadable.push(format!(
                        "{at}: a thread named after an argument of {carrier}, which the scan finds \
                         no call of"
                    ));
                }
                for (caller, argument) in calls {
                    self.read(caller, argument, depth + 1);
                }
            }
            Ok(Value::Parameter { .. }) => self.scan.unreadable.push(format!(
                "{at}: a thread name passed through more than {MAX_DEPTH} functions"
            )),
            Err(what) => self
                .scan
                .unreadable
                .push(format!("{at}: a thread name the scan cannot read: {what}")),
        }
    }

    /// What the expression `expr` of file `file` is.
    fn value(&self, file: usize, expr: Range<usize>) -> Result<Value, String> {
        let source = &self.sources[file];
        let text = || {
            expr.clone()
                .map(|at| match source.kind(at) {
                    Some(Kind::Ident(word)) => word.clone(),
                    Some(Kind::Str(text)) => format!("{text:?}"),
                    Some(Kind::Punct(c)) => c.to_string(),
                    _ => "…".to_owned(),
                })
                .collect::<String>()
        };
        let mut expr = expr.clone();
        // `&x`, `x.into()`, `x.to_owned()`, `x.to_string()`, `x.clone()`: x.
        while source.is(expr.start, '&') {
            expr.start += 1;
        }
        while expr.len() >= 4
            && source.is(expr.end - 4, '.')
            && matches!(
                source.ident(expr.end - 3),
                Some("into" | "to_owned" | "to_string" | "clone")
            )
            && source.is(expr.end - 2, '(')
            && source.is(expr.end - 1, ')')
        {
            expr.end -= 4;
        }
        // `String::from(x)`: x.
        if source.path_at(expr.start, &["String", "from"])
            && source.is(expr.start + 4, '(')
            && source.closing(expr.start + 4) == Some(expr.end - 1)
        {
            return self.value(file, expr.start + 5..expr.end - 1);
        }
        // `format!("…", …)`: the format string with what the scan reads of the arguments.
        if source.ident(expr.start) == Some("format")
            && source.is(expr.start + 1, '!')
            && source.is(expr.start + 2, '(')
            && source.closing(expr.start + 2) == Some(expr.end - 1)
        {
            return self
                .format(file, expr.start + 3..expr.end - 1)
                .map(Value::Texts);
        }
        match expr.len() {
            1 => match source.kind(expr.start) {
                Some(Kind::Str(text)) => return Ok(Value::Texts(vec![text.clone()])),
                Some(Kind::Ident(word)) => {
                    if let Some(value) = self.parameter(file, expr.start, word) {
                        return Ok(value);
                    }
                    if let Some(texts) = self.constants.get(word) {
                        return Ok(Value::Texts(texts.clone()));
                    }
                }
                _ => {}
            },
            // `module::CONSTANT`.
            n if n >= 4 && n % 3 == 1 => {
                let words: Vec<&str> = (0..=n / 3)
                    .filter_map(|k| source.ident(expr.start + 3 * k))
                    .collect();
                if words.len() == n / 3 + 1
                    && source.path_at(expr.start, &words)
                    && let Some(texts) = words.last().and_then(|last| self.constants.get(*last))
                {
                    return Ok(Value::Texts(texts.clone()));
                }
            }
            _ => {}
        }
        Err(text())
    }

    /// `word` at token `at` of file `file`, when it is a parameter of the function it is in.
    fn parameter(&self, file: usize, at: usize, word: &str) -> Option<Value> {
        let (function, found) = self.functions[file]
            .iter()
            .enumerate()
            .filter(|(_, function)| function.body.contains(&at))
            .min_by_key(|(_, function)| function.body.len())?;
        let index = found.params.iter().position(|param| param == word)?;
        Some(Value::Parameter {
            file,
            function,
            index,
        })
    }

    /// The texts of `format!`'s arguments `args`: its format string with each `{…}` replaced by
    /// what the scan reads of its argument, or left as `{}`. A name that starts with something
    /// the scan cannot read is unreadable.
    fn format(&self, file: usize, args: Range<usize>) -> Result<Vec<String>, String> {
        let source = &self.sources[file];
        let parts = source.split(args);
        let Some(Kind::Str(template)) = parts.first().and_then(|first| {
            (first.len() == 1)
                .then(|| source.kind(first.start))
                .flatten()
        }) else {
            return Err("format! with no format string".to_owned());
        };
        let mut positional = parts[1..].iter().filter(|part| {
            !(part.len() >= 2
                && source.ident(part.start).is_some()
                && source.is(part.start + 1, '='))
        });
        let named = |name: &str| {
            parts[1..].iter().find_map(|part| {
                (source.ident(part.start) == Some(name) && source.is(part.start + 1, '='))
                    .then(|| part.start + 2..part.end)
            })
        };
        let mut texts = vec![String::new()];
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            let piece = match c {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    vec!["{".to_owned()]
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    vec!["}".to_owned()]
                }
                '{' => {
                    let mut inside = String::new();
                    for c in chars.by_ref() {
                        if c == '}' {
                            break;
                        }
                        inside.push(c);
                    }
                    let name = inside.split(':').next().unwrap_or_default();
                    let unread = || vec!["{}".to_owned()];
                    let read = |range: Range<usize>| match self.value(file, range) {
                        Ok(Value::Texts(values)) => values,
                        _ => unread(),
                    };
                    if name.is_empty() {
                        positional.next().cloned().map_or_else(unread, read)
                    } else if let Ok(index) = name.parse::<usize>() {
                        parts.get(index + 1).cloned().map_or_else(unread, read)
                    } else if let Some(argument) = named(name) {
                        read(argument)
                    } else {
                        // An inline capture, `{NAME}`, reads as the identifier would.
                        self.constants.get(name).cloned().unwrap_or_else(unread)
                    }
                }
                c => vec![c.to_string()],
            };
            texts = texts
                .iter()
                .flat_map(|text| piece.iter().map(move |piece| format!("{text}{piece}")))
                .collect();
        }
        if texts.iter().any(|text| text.starts_with("{}")) {
            return Err(format!(
                "format!({template:?}, …) starts with what it cannot read"
            ));
        }
        Ok(texts)
    }

    /// The argument `index` of every call of function `function` of file `defined`: a call by a
    /// path through its module, a call in its own file, a call in a file that imports it, a call
    /// as a method if it takes `self`.
    fn calls(&self, defined: usize, function: usize, index: usize) -> Vec<(usize, Range<usize>)> {
        let target = &self.functions[defined][function];
        let module = module_of(&self.sources[defined].path);
        let mut found = Vec::new();
        for (file, source) in self.sources.iter().enumerate() {
            let imported = file == defined || imports(source, &module, &target.name);
            for at in 1..source.tokens.len() {
                if source.ident(at) != Some(target.name.as_str()) || !source.is(at + 1, '(') {
                    continue;
                }
                let by = if target.takes_self {
                    source.is(at - 1, '.')
                } else if source.is(at - 1, ':') {
                    at >= 3
                        && source.is(at - 2, ':')
                        && source.ident(at - 3) == Some(module.as_str())
                } else {
                    imported && !source.is(at - 1, '.') && source.ident(at - 1) != Some("fn")
                };
                let Some(close) = source.closing(at + 1) else {
                    continue;
                };
                if by && let Some(argument) = source.split(at + 2..close).get(index) {
                    found.push((file, argument.clone()));
                }
            }
        }
        found
    }
}

/// Whether `source` imports `item` from the module `module`: `module::item` in a `use`, or
/// `module::{…, item, …}`.
fn imports(source: &Source, module: &str, item: &str) -> bool {
    (0..source.tokens.len()).any(|at| {
        if source.ident(at) != Some(module) || !source.is(at + 1, ':') || !source.is(at + 2, ':') {
            return false;
        }
        if source.ident(at + 3) == Some(item) {
            return !source.is(at + 4, '(') && !source.is(at + 4, ':');
        }
        source.is(at + 3, '{')
            && source.closing(at + 3).is_some_and(|close| {
                source
                    .split(at + 4..close)
                    .iter()
                    .any(|part| source.ident(part.start) == Some(item) && part.len() <= 3)
            })
    })
}

/// Scan `sources`.
fn scan(sources: &[Source]) -> Scan {
    Scanner::new(sources).run()
}

// ---------------------------------------------------------------------------------------------
// The workspace
// ---------------------------------------------------------------------------------------------

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

/// Every crate's `src`, read.
fn workspace() -> Vec<Source> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates).expect("the crates are listed") {
        rust_sources(&entry.expect("a crate").path().join("src"), &mut files);
    }
    assert!(files.len() > 100, "only {} source files found", files.len());
    files
        .iter()
        .map(|file| {
            let src = std::fs::read_to_string(file)
                .unwrap_or_else(|err| panic!("{}: {err}", file.display()));
            let path = file.strip_prefix(&crates).unwrap_or(file);
            Source::new(&path.to_string_lossy().replace('\\', "/"), &src)
        })
        .collect()
}

/// The names in `scan` that carry the old name, in any case.
fn old_names(scan: &Scan) -> Vec<String> {
    scan.names
        .iter()
        .filter(|named| named.name.to_lowercase().contains("fxsound"))
        .map(|named| format!("{}: {:?}", named.at, named.name))
        .collect()
}

#[test]
fn every_thread_crossfx_names_says_crossfx() {
    let scan = scan(&workspace());
    assert!(
        scan.unreadable.is_empty(),
        "names the scan cannot read, so cannot vouch for:\n  {}",
        scan.unreadable.join("\n  ")
    );
    let old = old_names(&scan);
    assert!(
        old.is_empty(),
        "named for the program's old name:\n  {}",
        old.join("\n  ")
    );
    // And the scan still reads each way a name is given: a builder's literal, one through
    // `.into()`, a `format!`, and the carrier `wake::forward` names.
    for expected in [
        "crossfx-audio",
        "crossfx-fonts",
        "crossfx-{}",
        "crossfx-audio-wake",
    ] {
        assert!(
            scan.names.iter().any(|named| named.name == expected),
            "the scan no longer finds {expected:?}, so it may miss an old name too: {:?}",
            scan.names
        );
    }
    assert!(
        scan.builders >= 10,
        "only {} thread builders found",
        scan.builders
    );
}

/// The scan of one file, `src`, at `path`.
fn scan_of(files: &[(&str, &str)]) -> Scan {
    let sources: Vec<Source> = files
        .iter()
        .map(|(path, src)| Source::new(path, src))
        .collect();
    scan(&sources)
}

#[test]
fn a_name_is_read_through_a_constant_a_conversion_format_and_a_carrier() {
    let scan = scan_of(&[
        (
            "app/src/fonts.rs",
            "const FONTS_THREAD: &str = \"fxsound-fonts\";\n\
             fn a() { std::thread::Builder::new().name(FONTS_THREAD.into()).spawn(f); }\n\
             fn b() {\n\
                 thread::Builder::new().name(String::from(\"FxSound-picker\")).spawn(f);\n\
             }\n\
             fn c() {\n\
                 thread::Builder::new()\n\
                     .stack_size(1)\n\
                     .name(format!(\"fxsound-{}\", key))\n\
                     .spawn(f);\n\
             }\n\
             fn d() {\n\
                 thread::Builder::new().name(format!(\"{}-{PART}\", \"crossfx\")).spawn(f);\n\
             }\n\
             const PART: &str = \"fxsound\";\n",
        ),
        (
            "app/src/wake.rs",
            "pub fn forward<T: Send + 'static>(\n\
                 name: &str,\n\
                 source: Receiver<T>,\n\
             ) -> Receiver<T> {\n\
                 let spawned = std::thread::Builder::new().name(name.to_owned()).spawn(|| ());\n\
             }\n\
             fn test() { let r = forward(\"crossfx-test-carrier\", source); }\n",
        ),
        (
            "app/src/audio_link.rs",
            "fn link() {\n\
                 let n = wake::forward(r#\"fxsound-audio-wake\"#, engine.notifications());\n\
             }\n",
        ),
    ]);
    assert!(scan.unreadable.is_empty(), "{:?}", scan.unreadable);
    let names: Vec<&str> = scan.names.iter().map(|named| named.name.as_str()).collect();
    for expected in [
        "fxsound-fonts",
        "FxSound-picker",
        "fxsound-{}",
        "crossfx-fxsound",
        "crossfx-test-carrier",
        "fxsound-audio-wake",
    ] {
        assert!(names.contains(&expected), "{expected:?} not in {names:?}");
    }
    assert_eq!(old_names(&scan).len(), 5, "{names:?}");
    assert_eq!(scan.builders, 5);
}

#[test]
fn a_name_the_scan_cannot_read_and_a_builder_that_names_nothing_fail_it() {
    let scan = scan_of(&[(
        "app/src/workers.rs",
        "fn a() { thread::Builder::new().name(make_name()).spawn(f); }\n\
         fn b() { let builder = thread::Builder::new(); builder.spawn(f); }\n\
         fn c() { thread::Builder::new().name(format!(\"{}-worker\", prefix())).spawn(f); }\n\
         fn d(name: &str) { thread::Builder::new().name(name.into()).spawn(f); }\n",
    )]);
    let said = scan.unreadable.join("\n");
    assert_eq!(scan.unreadable.len(), 4, "{said}");
    assert!(
        said.contains("workers.rs:1: a thread name the scan cannot read: make_name()"),
        "{said}"
    );
    assert!(
        said.contains("workers.rs:2: a thread builder that does not name its thread"),
        "{said}"
    );
    assert!(said.contains("starts with what it cannot read"), "{said}");
    assert!(
        said.contains("an argument of d, which the scan finds no call of"),
        "{said}"
    );
}

#[test]
fn a_name_that_is_not_a_threads_or_is_in_a_comment_is_not_read() {
    let scan = scan_of(&[(
        "audio/src/mock.rs",
        "// thread::Builder::new().name(\"fxsound-commented\".into()).spawn(f);\n\
         /* thread::Builder::new().name(\"fxsound-blocked\".into()) */\n\
         fn devices() { Device::builder().name(\"FxSound Speakers\").build(); }\n\
         fn bus() { zbus::Builder::address(a).name(\"org.freedesktop.Notifications\"); }\n\
         fn files() { tempfile::Builder::new().prefix(\"fxsound-\").tempdir(); }\n\
         fn text() -> &'static str { \"thread::Builder::new().name(\\\"fxsound-quoted\\\")\" }\n\
         fn real() { std::thread::Builder::new().name(\"crossfx-real\".to_owned()).spawn(f); }\n",
    )]);
    assert!(scan.unreadable.is_empty(), "{:?}", scan.unreadable);
    assert_eq!(
        scan.names,
        [Named {
            name: "crossfx-real".to_owned(),
            at: "audio/src/mock.rs:7".to_owned(),
        }]
    );
}

#[test]
fn an_imported_builder_and_an_imported_carrier_are_read_too() {
    let scan = scan_of(&[
        (
            "app/src/pool.rs",
            "use std::thread::{self, Builder};\n\
             fn a() { Builder::new().name(\"fxsound-pool\".into()).spawn(f); }\n\
             pub(crate) fn start(label: &str) {\n\
                 Builder::new().name(label.to_owned()).spawn(f);\n\
             }\n",
        ),
        (
            "app/src/main.rs",
            "use crate::pool::start;\n\
             fn main() {\n\
                 start(\"fxsound-main\");\n\
                 other::start(\"elsewhere\");\n\
             }\n\
             fn forward(x: u32) {}\n\
             fn t() { forward(1); }\n",
        ),
    ]);
    assert!(scan.unreadable.is_empty(), "{:?}", scan.unreadable);
    let names: Vec<&str> = scan.names.iter().map(|named| named.name.as_str()).collect();
    assert_eq!(names, ["fxsound-pool", "fxsound-main"]);
}
