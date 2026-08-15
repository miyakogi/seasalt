use std::path::{Path, PathBuf};

const OPERATORS: &[&str] = &[
    "|", "||", "|&", "&&", "&", ";", ";;", "<", ">", ">>", "<<", "(", ")", "{", "}", "!", "$",
];

/// Returns the arguments of the command that exist relative to cwd at
/// record time (fish's required_paths equivalent). The command name,
/// flags, shell operators, command substitutions and `.`/`..` are not
/// treated as paths. Arguments that do not exist are not constraints,
/// so arguments like `echo hello` or `git push` always keep the
/// candidate visible.
pub fn required_paths(cwd: &str, cmd: &str) -> Vec<String> {
    let tokens = tokenize(cmd);
    let mut out = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        if i == 0 {
            continue; // command name
        }
        if tok.is_empty() || tok.starts_with('-') || is_operator(tok) {
            continue;
        }
        if tok.contains("$(") || tok.contains('`') {
            continue; // command substitutions are not expanded
        }
        let path = unquote(tok);
        if path.is_empty() || matches!(path.as_str(), "." | ".." | "./" | "../") {
            continue;
        }
        if resolve(cwd, &path).exists() {
            out.push(path);
        }
    }
    out
}

/// Returns true if all the required paths (NUL-separated) exist relative to the current cwd
pub fn valid(cwd: &str, paths: &str) -> bool {
    if paths.is_empty() {
        return true;
    }
    paths
        .split('\0')
        .all(|p| p.is_empty() || resolve(cwd, p).exists())
}

fn resolve(cwd: &str, path: &str) -> PathBuf {
    if path.starts_with('/') {
        PathBuf::from(path)
    } else {
        Path::new(cwd).join(path)
    }
}

fn is_operator(tok: &str) -> bool {
    OPERATORS.contains(&tok)
}

/// Removes the surrounding matching quotes
fn unquote(tok: &str) -> String {
    if tok.len() >= 2 {
        let first = tok.chars().next().unwrap();
        let last = tok.chars().last().unwrap();
        if (first == '"' || first == '\'') && first == last {
            return tok[1..tok.len() - 1].to_string();
        }
    }
    tok.to_string()
}

/// Tokenizes by whitespace, honoring quotes and backslashes. A
/// backslash escapes the next character (the backslash itself is dropped).
fn tokenize(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in cmd.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\\' => escaped = true,
                '\'' | '"' => {
                    quote = Some(c);
                    cur.push(c);
                }
                c if c.is_whitespace() => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                _ => cur.push(c),
            },
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
