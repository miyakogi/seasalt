use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn init_bash_output_is_valid_bash_syntax() {
    let out = Command::new(env!("CARGO_BIN_EXE_seasalt"))
        .args(["init", "bash"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(!out.stdout.is_empty());

    let mut child = Command::new("bash")
        .args(["-n"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&out.stdout)
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "bash -n が失敗: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn init_bash_survives_eval_command_substitution() {
    let bin = env!("CARGO_BIN_EXE_seasalt");
    let script = format!(
        r#"set -euo pipefail
SEASALT_BIN={bin}
SEASALT_DATA_DIR=$(mktemp -d)
blehook() {{ :; }}
_ble_complete_auto_source=(history syntax)
eval $("$SEASALT_BIN" init bash)
declare -F _seasalt_preexec >/dev/null
declare -F _seasalt_precmd >/dev/null
"#
    );
    let out = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(
        out.status.success(),
        "eval に失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn init_bash_rejects_unknown_shell() {
    let out = Command::new(env!("CARGO_BIN_EXE_seasalt"))
        .args(["init", "zsh"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}
