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
        "bash -n failed: {}",
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
ble/util/idle.push() {{ :; }}
eval "$("$SEASALT_BIN" init bash)"
declare -F _seasalt_preexec >/dev/null
declare -F _seasalt_precmd >/dev/null
"#
    );
    let out = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(
        out.status.success(),
        "eval failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn init_bash_fails_on_unquoted_eval() {
    let bin = env!("CARGO_BIN_EXE_seasalt");
    // Unquoted eval (eval $(...)) breaks the snippet via word-splitting
    // and fails with a syntax error; the quoted form (eval "$(...)") is required.
    let script = format!(
        r#"set +e
SEASALT_BIN={bin}
out=$(eval $("$SEASALT_BIN" init bash) 2>&1)
rc=$?
[[ $rc -ne 0 ]] || exit 1
[[ -n $out ]]
"#
    );
    let out = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(
        out.status.success(),
        "unquoted eval did not fail: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn init_bash_reorders_auto_source_after_idle() {
    let bin = env!("CARGO_BIN_EXE_seasalt");
    let script = format!(
        r#"set -euo pipefail
SEASALT_BIN={bin}
SEASALT_DATA_DIR=$(mktemp -d)
BASHER_HOOKS=()
BASHER_IDLE_TASKS=()
blehook() {{ BASHER_HOOKS+=("$*"); }}
ble/util/idle.push() {{ BASHER_IDLE_TASKS+=("$1"); }}
# Real ble.sh runs idle tasks inside a function, so the test does the same
fire_idle_task() {{ eval "$1"; }}
unset _ble_complete_auto_source
eval "$("$SEASALT_BIN" init bash)"
[[ ${{#BASHER_HOOKS[@]}} -eq 2 ]]
[[ ${{#BASHER_IDLE_TASKS[@]}} -eq 1 ]]
# Running the idle task with _ble_complete_auto_source unset initializes it safely
fire_idle_task "${{BASHER_IDLE_TASKS[0]}}"
[[ " ${{_ble_complete_auto_source[*]}} " == " seasalt syntax " ]]
# After core-complete load (unconditional reset) and atuin onload registration,
# the idle task puts seasalt first and removes the other sources
_ble_complete_auto_source=(atuin-history history syntax)
fire_idle_task "${{BASHER_IDLE_TASKS[0]}}"
[[ " ${{_ble_complete_auto_source[*]}} " == " seasalt syntax " ]]
# Re-eval after startup stays idempotent without duplicating anything
eval "$("$SEASALT_BIN" init bash)"
[[ ${{#BASHER_HOOKS[@]}} -eq 2 ]]
[[ ${{#BASHER_IDLE_TASKS[@]}} -eq 2 ]]
fire_idle_task "${{BASHER_IDLE_TASKS[1]}}"
[[ " ${{_ble_complete_auto_source[*]}} " == " seasalt syntax " ]]
"#
    );
    let out = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(
        out.status.success(),
        "auto-complete source reordering failed: {}",
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
    // Interactive commands report errors to stderr
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unsupported shell: zsh"), "stderr: {err}");
}
