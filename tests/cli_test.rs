use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_seasalt"))
}

fn temp_data_dir() -> std::path::PathBuf {
    // テストごとにユニークなディレクトリにする(並列実行時に他テストと DB を共有しないため)
    let name = std::thread::current().name().unwrap_or("t").to_string();
    let dir = std::env::temp_dir().join(format!("seasalt-cli-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir); // テスト用の一時ディレクトリなので削除してよい
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn record_then_exit_then_search() {
    let dir = temp_data_dir();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            "/tmp/x",
            "--session",
            "s1",
            "--",
            "echo hello",
        ])
        .status()
        .unwrap();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["exit", "--session", "s1", "--last-id", "1", "--code", "0"])
        .status()
        .unwrap();

    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--tsv", "hello"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    // tsv 形式: id\tcwd\tcmd\texit_code\tstarted_at (started_at は実時刻のため後半のみ確認)
    let fields: Vec<&str> = text.trim().split('\t').collect();
    assert_eq!(fields[0..4], ["1", "/tmp/x", "echo hello", "0"]);
    assert!(!fields[4].is_empty());
}

#[test]
fn search_filters_by_cwd() {
    let dir = temp_data_dir();
    for (cwd, cmd) in [("/a", "one"), ("/b", "two")] {
        bin()
            .env("SEASALT_DATA_DIR", &dir)
            .args(["record", "--cwd", cwd, "--session", "s1", "--", cmd])
            .status()
            .unwrap();
    }
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--cwd", "/a", "o"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.trim(), "one");
}

#[test]
fn suggest_end_to_end() {
    let dir = temp_data_dir();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            "/proj",
            "--session",
            "s1",
            "--",
            "cargo build",
        ])
        .status()
        .unwrap();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["suggest", "--cwd", "/proj", "--", "cargo"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "cargo build");

    // 候補なし → 空出力・成功終了
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["suggest", "--cwd", "/proj", "--", "nothing"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"");
}

#[test]
fn record_prints_row_id() {
    let dir = temp_data_dir();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/x", "--session", "s1", "--", "cmd"])
        .output()
        .unwrap();
    let id: i64 = String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(id > 0);
}
