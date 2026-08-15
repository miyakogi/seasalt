use seasalt::paths;

fn temp_dir() -> std::path::PathBuf {
    let name = std::thread::current().name().unwrap_or("t").to_string();
    let dir = std::env::temp_dir().join(format!("seasalt-paths-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir); // テスト用の一時ディレクトリなので削除してよい
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn detects_existing_files_only() {
    let dir = temp_dir();
    std::fs::write(dir.join("a.txt"), "x").unwrap();
    let cwd = dir.to_str().unwrap();

    // a.txt のみ存在 → a.txt だけ返す
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

    // 先頭トークン (コマンド名) はパス判定しない。ファイル名 nvim があっても無関係
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

    // コマンド置換を含むトークンは判定しない
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

    // 存在するパス → true
    assert!(paths::valid(cwd, "a.txt"));
    let abs = dir.join("a.txt").to_str().unwrap().to_string();
    assert!(paths::valid(cwd, &abs));
    // 存在しないパス → false
    assert!(!paths::valid(cwd, "b.txt"));
    // 空は常に true
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
