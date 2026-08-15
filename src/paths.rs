use std::path::{Path, PathBuf};

const OPERATORS: &[&str] = &[
    "|", "||", "|&", "&&", "&", ";", ";;", "<", ">", ">>", "<<", "(", ")", "{", "}", "!", "$",
];

/// コマンドの引数のうち、記録時点で cwd 基準に存在するものを返す
/// (fish の required_paths 相当)。コマンド名・フラグ・シェル演算子・
/// コマンド置換・`.`/`..` はパス判定しない。存在しない引数は制約にならないため
/// `echo hello` や `git push` のような引数は常に候補に残る。
pub fn required_paths(cwd: &str, cmd: &str) -> Vec<String> {
    let tokens = tokenize(cmd);
    let mut out = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        if i == 0 {
            continue; // コマンド名
        }
        if tok.is_empty() || tok.starts_with('-') || is_operator(tok) {
            continue;
        }
        if tok.contains("$(") || tok.contains('`') {
            continue; // コマンド置換は展開しない
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

/// 保存済みの必須パス (NUL 区切り) が全て現在の cwd 基準で存在すれば true
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

/// 周囲の対応するクォートを外す
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

/// クォートとバックスラッシュを考慮した空白区切りのトークン化。
/// バックスラッシュはエスケープ文字として次の 1 文字に付与される (バックスラッシュ自体は消える)。
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
