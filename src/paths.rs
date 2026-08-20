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
    let tokens = shlex::split(cmd).unwrap_or_default();
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
        let path = tok.as_str();
        if path.is_empty() || matches!(path, "." | ".." | "./" | "../") {
            continue;
        }
        if resolve(cwd, path).exists() {
            out.push(path.to_string());
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
