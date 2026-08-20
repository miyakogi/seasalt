use seasalt::paths;

fn temp_dir() -> std::path::PathBuf {
    let name = std::thread::current().name().unwrap_or("t").to_string();
    let dir = std::env::temp_dir().join(format!("seasalt-paths-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir); // temp dir for the test; safe to delete
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn detects_existing_files_only() {
    let dir = temp_dir();
    std::fs::write(dir.join("a.txt"), "x").unwrap();
    let cwd = dir.to_str().unwrap();

    // Only a.txt exists -> returns only a.txt
    assert_eq!(
        paths::required_paths(cwd, "nvim a.txt b.txt"),
        vec!["a.txt"]
    );

    std::fs::write(dir.join("b.txt"), "x").unwrap();
    assert_eq!(
        paths::required_paths(cwd, "nvim a.txt b.txt"),
        vec!["a.txt", "b.txt"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn command_word_is_not_a_path() {
    let dir = temp_dir();
    std::fs::write(dir.join("nvim"), "x").unwrap();
    let cwd = dir.to_str().unwrap();

    // The first token (command name) is not judged as a path; a file named nvim is irrelevant
    assert_eq!(
        paths::required_paths(cwd, "nvim a.txt"),
        Vec::<String>::new()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn skips_flags_operators_and_cmdsubst() {
    let dir = temp_dir();
    let cwd = dir.to_str().unwrap();

    let got = paths::required_paths(cwd, "git push origin main && echo hi > out.txt");
    assert_eq!(got, Vec::<String>::new());

    // Tokens containing command substitutions are not judged
    assert_eq!(
        paths::required_paths(cwd, "nvim $(date).txt"),
        Vec::<String>::new()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handles_quoted_and_escaped_args() {
    let dir = temp_dir();
    std::fs::write(dir.join("my file.txt"), "x").unwrap();
    std::fs::write(dir.join("esc.txt"), "x").unwrap();
    let cwd = dir.to_str().unwrap();

    assert_eq!(
        paths::required_paths(cwd, "nvim \"my file.txt\""),
        vec!["my file.txt"]
    );
    assert_eq!(
        paths::required_paths(cwd, "nvim esc\\.txt"),
        vec!["esc.txt"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn skips_dot_dirs() {
    let dir = temp_dir();
    let cwd = dir.to_str().unwrap();

    assert_eq!(
        paths::required_paths(cwd, "cd . && cd .. && ls"),
        Vec::<String>::new()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn detects_directories_too() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("proj")).unwrap();
    let cwd = dir.to_str().unwrap();

    assert_eq!(paths::required_paths(cwd, "cd proj"), vec!["proj"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn valid_checks_against_current_cwd() {
    let dir = temp_dir();
    std::fs::write(dir.join("a.txt"), "x").unwrap();
    let cwd = dir.to_str().unwrap();

    // Existing path -> true
    assert!(paths::valid(cwd, "a.txt"));
    let abs = dir.join("a.txt").to_str().unwrap().to_string();
    assert!(paths::valid(cwd, &abs));
    // Nonexistent path -> false
    assert!(!paths::valid(cwd, "b.txt"));
    // Empty is always true
    assert!(paths::valid(cwd, ""));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn valid_multiple_paths_all_must_exist() {
    let dir = temp_dir();
    std::fs::write(dir.join("a.txt"), "x").unwrap();
    let cwd = dir.to_str().unwrap();

    assert!(paths::valid(cwd, "a.txt"));
    assert!(paths::valid(cwd, "a.txt\0a.txt"));
    assert!(!paths::valid(cwd, "a.txt\0b.txt"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handles_escaped_quotes_inside_quotes() {
    let dir = temp_dir();
    // Create a file with a quote in its name (unlikely but valid on Unix)
    let name = "a\"b.txt";
    std::fs::write(dir.join(name), "x").unwrap();
    let cwd = dir.to_str().unwrap();

    // Double-quoted arg containing an escaped double quote
    let cmd = format!("nvim \"{}\"", "a\\\"b.txt");
    // tokenize should keep the escaped quote, unquote should strip outer quotes
    // and leave a"b.txt as the path
    assert_eq!(paths::required_paths(cwd, &cmd), vec![name.to_string()]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handles_escaped_single_quote() {
    let dir = temp_dir();
    let cwd = dir.to_str().unwrap();
    // Single-quoted arg with escaped single quote (shell would be 'a'\''b' but we test simple)
    // Our tokenizer treats backslash as escape even inside single quotes for simplicity
    std::fs::write(dir.join("a'b.txt"), "x").unwrap();
    let cmd = "nvim 'a\\'b.txt'";
    assert_eq!(paths::required_paths(cwd, cmd), vec!["a'b.txt".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}
