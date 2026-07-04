use std::process::Command;

#[test]
fn eval_without_corpus_is_skip_safe() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("eval")
        .env_remove("YESVOX_EVAL_CORPUS")
        .output()
        .expect("run xtask eval");

    assert!(
        output.status.success(),
        "eval without a local corpus must be CI-safe; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("YESVOX_EVAL_CORPUS"),
        "skip notice should name the env var; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("eval: skipped"),
        "skip notice should be explicit; stderr:\n{stderr}"
    );
}
