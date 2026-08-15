use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_seasalt"))
}

fn temp_data_dir() -> std::path::PathBuf {
    // Unique directory per test (parallel runs must not share the DB)
    let name = std::thread::current().name().unwrap_or("t").to_string();
    let dir = std::env::temp_dir().join(format!("seasalt-cli-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir); // temp dir for the test; safe to delete
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
        .args(["exit", "--last-id", "1", "--code", "0"])
        .status()
        .unwrap();

    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "--tsv", "hello"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    // tsv format: id\tcwd\tcmd\texit_code\tstarted_at (started_at is the real time, so only check the rest)
    let fields: Vec<&str> = text.trim().split('\t').collect();
    assert_eq!(fields[0..4], ["1", "/tmp/x", "echo hello", "0"]);
    assert!(!fields[4].is_empty());
}

#[test]
fn record_dedups_identical_command() {
    let dir = temp_data_dir();
    let first = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            "/x",
            "--session",
            "s1",
            "--",
            "echo hello",
        ])
        .output()
        .unwrap();
    let second = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            "/x",
            "--session",
            "s2",
            "--",
            "echo hello",
        ])
        .output()
        .unwrap();
    let id1 = String::from_utf8(first.stdout).unwrap();
    let id2 = String::from_utf8(second.stdout).unwrap();
    assert_eq!(id1.trim(), id2.trim());

    // Only one row remains in history
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "--tsv", "hello"])
        .output()
        .unwrap();
    assert_eq!(out.stdout.iter().filter(|&&b| b == b'\n').count(), 1);
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
    // Default output is id<TAB>cmd
    assert_eq!(text.trim(), "1\tone");
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

    // No candidate -> empty output, successful exit
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

#[test]
fn record_stores_existing_paths() {
    let dir = temp_data_dir();
    let name = std::thread::current().name().unwrap_or("t").to_string();
    let files =
        std::env::temp_dir().join(format!("seasalt-cli-files-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&files);
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("a.txt"), "x").unwrap();
    let cwd = files.to_str().unwrap();

    // Existing file -> saved in paths
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            cwd,
            "--session",
            "s1",
            "--",
            "nvim a.txt",
        ])
        .status()
        .unwrap();
    // Nonexistent file -> empty paths
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            cwd,
            "--session",
            "s1",
            "--",
            "echo hello",
        ])
        .status()
        .unwrap();

    let conn = rusqlite::Connection::open(dir.join("history.sqlite3")).unwrap();
    let paths: String = conn
        .query_row(
            "SELECT paths FROM history WHERE cmd = 'nvim a.txt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(paths, "a.txt");
    let paths: String = conn
        .query_row(
            "SELECT paths FROM history WHERE cmd = 'echo hello'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(paths, "");

    let _ = std::fs::remove_dir_all(&files);
}

#[test]
fn suggest_filters_deleted_files() {
    let dir = temp_data_dir();
    let name = std::thread::current().name().unwrap_or("t").to_string();
    let files =
        std::env::temp_dir().join(format!("seasalt-cli-files-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&files);
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("a.txt"), "x").unwrap();
    let cwd = files.to_str().unwrap();

    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            cwd,
            "--session",
            "s1",
            "--",
            "nvim a.txt",
        ])
        .status()
        .unwrap();

    // Suggested while the file exists
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["suggest", "--cwd", cwd, "--", "nvim"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "nvim a.txt");

    // Not suggested once deleted
    std::fs::remove_file(files.join("a.txt")).unwrap();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["suggest", "--cwd", cwd, "--", "nvim"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"");

    let _ = std::fs::remove_dir_all(&files);
}

#[test]
fn delete_removes_history_entries() {
    let dir = temp_data_dir();
    for cmd in ["one", "two", "three"] {
        bin()
            .env("SEASALT_DATA_DIR", &dir)
            .args(["record", "--cwd", "/a", "--session", "s1", "--", cmd])
            .status()
            .unwrap();
    }

    // Multiple ids are deleted at once; nothing is printed on success
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["delete", "1", "3"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"");

    // Deleted commands disappear from both search and suggest
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "one"])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"");
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["suggest", "--cwd", "/a", "--", "one"])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"");

    // The remaining row is visible in the default id<TAB>cmd output
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "two"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "2\ttwo");

    // Nonexistent ids are silently accepted
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["delete", "999"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"");
}

#[test]
fn record_ignores_leading_whitespace_commands() {
    let dir = temp_data_dir();

    // Commands starting with space/tab are not recorded (exit 0, no output)
    for cmd in ["  ls -la", "\tgit status"] {
        let out = bin()
            .env("SEASALT_DATA_DIR", &dir)
            .args(["record", "--cwd", "/a", "--session", "s1", "--", cmd])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"");
    }
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "ls"])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"");

    // Normal commands are still recorded
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["record", "--cwd", "/a", "--session", "s1", "--", "ls -la"])
        .status()
        .unwrap();
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "ls"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "1\tls -la");
}

#[test]
fn init_does_not_create_data_dir() {
    let dir = std::env::temp_dir().join(format!(
        "seasalt-cli-init-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("t")
    ));
    let _ = std::fs::remove_dir_all(&dir);

    let out = bin()
        .env_remove("SEASALT_DATA_DIR")
        .env("XDG_DATA_HOME", &dir)
        .args(["init", "bash"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!out.stdout.is_empty());
    // init does not create the DB (it only prints the snippet)
    assert!(!dir.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn record_failure_is_silent() {
    // record fails with an unwritable data directory, but because it is
    // called from hooks, nothing goes to stderr
    let out = bin()
        .env("SEASALT_DATA_DIR", "/proc/seasalt-test-readonly")
        .args(["record", "--cwd", "/x", "--session", "s1", "--", "echo hi"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(out.stderr, b"");
}

#[test]
fn search_defaults_to_current_directory() {
    let dir = temp_data_dir();
    let here = seasalt::search::default_cwd().unwrap();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            here.as_str(),
            "--session",
            "s1",
            "--",
            "unique-cmd-xyz",
        ])
        .status()
        .unwrap();
    bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args([
            "record",
            "--cwd",
            "/elsewhere",
            "--session",
            "s1",
            "--",
            "unique-cmd-other",
        ])
        .status()
        .unwrap();
    // Without --cwd the search is scoped to the process cwd
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "unique-cmd"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        "1\tunique-cmd-xyz"
    );
    // --all finds both
    let out = bin()
        .env("SEASALT_DATA_DIR", &dir)
        .args(["search", "--all", "unique-cmd"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap().lines().count(), 2);
}
