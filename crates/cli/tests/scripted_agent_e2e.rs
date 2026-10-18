//! End-to-end: record the scripted coding agent (examples/scripted-agent)
//! through a live collector, then fork it with the `replaykit` CLI using
//! each kind of patch and check that replays are hermetic and that patched
//! values really reach the executors.
//!
//! Needs `python3` and `git` on PATH (skips otherwise).

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use replaykit_core_model::{RunId, RunStatus, SpanId, SpanStatus, Value};
use replaykit_storage::{SqliteStorage, Storage};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn agent_dir() -> PathBuf {
    repo_root().join("examples/scripted-agent")
}

fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rk-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn start_collector(data_root: &Path) -> String {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let data_root = data_root.to_path_buf();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime
            .block_on(replaykit_collector::server::serve(addr, data_root))
            .unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while TcpStream::connect(addr).is_err() {
        assert!(Instant::now() < deadline, "collector did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    format!("http://{addr}")
}

/// Hash of every file in the live workspace (to prove replays never touch it).
fn tree_digest(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

struct Harness {
    data_root: PathBuf,
    sandbox_root: PathBuf,
    storage: Arc<SqliteStorage>,
    source: RunId,
}

impl Harness {
    fn cli(&self, args: &[&str]) -> serde_json::Value {
        let out = Command::new(env!("CARGO_BIN_EXE_replaykit"))
            .arg("--storage")
            .arg("sqlite")
            .arg("--data-root")
            .arg(&self.data_root)
            .arg("--sandbox-root")
            .arg(&self.sandbox_root)
            .arg("--json")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "replaykit {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn fork(&self, extra: &[&str], span: &str, patch_type: &str, replacement: &str) -> RunId {
        let mut args = extra.to_vec();
        args.extend([
            "replay",
            "fork",
            &self.source.0,
            "--span",
            span,
            "--patch-type",
            patch_type,
            "--replacement",
            replacement,
        ]);
        let view = self.cli(&args);
        RunId(view["target_run_id"].as_str().unwrap().to_owned())
    }

    fn span(&self, run: &RunId, span: &str) -> replaykit_core_model::SpanRecord {
        self.storage.get_span(run, &SpanId(span.into())).unwrap()
    }

    fn output(&self, run: &RunId, span: &str) -> String {
        let span = self.span(run, span);
        let id = span.output_artifact_ids.first().expect("span has output");
        String::from_utf8(self.storage.read_artifact_content(run, id).unwrap()).unwrap()
    }

    fn attr(&self, run: &RunId, span: &str, key: &str) -> String {
        match self.span(run, span).attributes.get(key) {
            Some(Value::Text(text)) => text.clone(),
            other => panic!("{span}.{key} = {other:?}"),
        }
    }

    fn diff_spans(&self, target: &RunId) -> Vec<String> {
        let diff = self.cli(&["runs", "diff", &self.source.0, &target.0]);
        assert!(diff["changed_span_count"].as_u64().unwrap() > 0);
        let record = self.storage.get_diff(&self.source, target).unwrap();
        match record.summary.get("span_diffs") {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| match item {
                    Value::Object(doc) => match doc.get("span_id_source") {
                        Some(Value::Text(id)) => Some(id.clone()),
                        _ => None,
                    },
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[test]
fn scripted_agent_forks_replay_hermetically_with_patched_inputs() {
    if !have("python3") || !have("git") {
        eprintln!("skipping: python3 and git are required");
        return;
    }
    let root = scratch("agent");
    let workspace = root.join("ws");
    let data_root = root.join("data");
    let sandbox_root = root.join("sandboxes");

    // Record.
    let status = Command::new("python3")
        .arg(agent_dir().join("agent.py"))
        .arg("make-repo")
        .arg(&workspace)
        .status()
        .unwrap();
    assert!(status.success());
    let collector = start_collector(&data_root);
    let out = Command::new("python3")
        .arg(agent_dir().join("agent.py"))
        .args(["run", "--collector", &collector, "--workspace"])
        .arg(&workspace)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let source = RunId(String::from_utf8(out.stdout).unwrap().trim().to_owned());

    let storage = Arc::new(SqliteStorage::open_with_data_root(&data_root).unwrap());
    let h = Harness {
        data_root: data_root.clone(),
        sandbox_root: sandbox_root.clone(),
        storage,
        source: source.clone(),
    };
    assert_eq!(
        h.storage.get_run(&source).unwrap().status,
        RunStatus::Failed
    );
    assert_eq!(h.storage.list_spans(&source).unwrap().len(), 15);
    assert!(
        h.output(&source, "test_repro")
            .contains("4 passed; 2 failed")
    );
    assert!(
        h.output(&source, "test_validate")
            .contains("5 passed; 1 failed")
    );

    // The recorded fix is still in the live tree: a non-hermetic replay of
    // the repro step would see it. Everything below must leave it untouched.
    let live_before = tree_digest(&workspace);
    assert!(String::from_utf8_lossy(&live_before["auth/session.py"]).contains("timeout=None"));

    // 1. Shell patch (command_override) on the repro step: the new command
    //    runs, in the workspace as it was before the fix.
    let repro = h.fork(
        &[],
        "test_repro",
        "command_override",
        "python3 run_tests.py tests/test_session.py",
    );
    assert_eq!(
        h.attr(&repro, "test_repro", "command"),
        "python3 run_tests.py tests/test_session.py"
    );
    let out = h.output(&repro, "test_repro");
    assert!(!out.contains("test_login"), "{out}");
    assert!(out.contains("test_session_timeout ... FAILED"), "{out}");
    assert!(out.contains("1 passed; 1 failed"), "{out}");
    assert!(h.diff_spans(&repro).contains(&"test_repro".to_string()));

    //    Re-running the recorded command reproduces the recorded 4/2, not
    //    the 5/1 the live tree would give.
    let rerun = h.fork(&[], "test_repro", "env_var_override", "PYTHONHASHSEED=0");
    assert!(
        h.output(&rerun, "test_repro")
            .contains("4 passed; 2 failed")
    );
    assert_eq!(
        h.span(&rerun, "test_repro").output_fingerprint,
        h.span(&source, "test_repro").output_fingerprint
    );

    // 2. Env patch on the validation step: the recorded fix passes once a
    //    pool timeout is configured, and the run re-derives to completed.
    let env = h.fork(
        &[],
        "test_validate",
        "env_var_override",
        "AUTH_POOL_TIMEOUT=0.05",
    );
    assert!(
        h.output(&env, "test_validate")
            .contains("result: OK. 6 passed")
    );
    assert_eq!(h.span(&env, "plan_validate").status, SpanStatus::Completed);
    assert_eq!(
        h.storage.get_run(&env).unwrap().status,
        RunStatus::Completed
    );
    // Containers re-derive their status; the divergence is the test step.
    let diff = h.cli(&["runs", "diff", &source.0, &env.0]);
    assert_eq!(diff["first_divergent_span_id"], "test_validate");

    // 3. Tool output patch: the narrowed test list flows into both test
    //    commands through their input bindings.
    let tool = h.fork(
        &[],
        "select_tests",
        "tool_output_override",
        "tests/test_session.py\n",
    );
    for span in ["test_repro", "test_validate"] {
        assert_eq!(
            h.attr(&tool, span, "command"),
            "python3 run_tests.py tests/test_session.py"
        );
    }
    assert!(h.output(&tool, "test_repro").contains("1 passed; 1 failed"));
    assert!(
        h.output(&tool, "test_validate")
            .contains("result: OK. 2 passed")
    );
    assert_eq!(
        h.storage.get_run(&tool).unwrap().status,
        RunStatus::Completed
    );

    // 4. Prompt patch: blocked up front without a model executor...
    let template = "You are fixing a bug in a Python repository.\n\nISSUE.md:\n{{read_issue}}\nauth/session.py:\n{{read_session}}\nReturn the full corrected file. Default the timeout to 2 seconds when none is configured.";
    let plan = h.cli(&[
        "replay",
        "plan",
        &source.0,
        "--span",
        "gen_fix",
        "--patch-type",
        "prompt_edit",
        "--replacement",
        template,
    ]);
    let blocked = plan["blocked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["span_id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert!(blocked.contains(&"gen_fix".to_string()), "{plan}");
    assert!(blocked.contains(&"test_validate".to_string()), "{plan}");
    let blocked_run = h.fork(&[], "gen_fix", "prompt_edit", template);
    let mut blocked_now = h
        .storage
        .list_spans(&blocked_run)
        .unwrap()
        .into_iter()
        .filter(|s| s.status == SpanStatus::Blocked)
        .map(|s| s.span_id.0)
        .collect::<Vec<_>>();
    let mut planned = blocked.clone();
    blocked_now.sort();
    planned.sort();
    assert_eq!(blocked_now, planned, "plan and execution must agree");

    //    ...and re-run end to end through the pluggable provider (the same
    //    scripted fixer the agent used).
    let fixer = format!("python3 {}", agent_dir().join("fixer.py").display());
    let llm = h.fork(
        &["--model-mode", "command", "--model-command", &fixer],
        "gen_fix",
        "prompt_edit",
        template,
    );
    assert!(
        h.attr(&llm, "gen_fix", "prompt")
            .contains("Default the timeout to 2 seconds")
    );
    assert!(
        h.attr(&llm, "gen_fix", "prompt")
            .contains("def init_session(pool):")
    );
    let fixed = h.output(&llm, "gen_fix");
    assert!(fixed.contains("\"AUTH_POOL_TIMEOUT\", \"2\""), "{fixed}");
    assert_eq!(h.attr(&llm, "write_fix", "content"), fixed);
    assert!(
        h.output(&llm, "test_validate")
            .contains("result: OK. 6 passed")
    );
    assert!(h.output(&llm, "diff_stat").contains("auth/session.py"));
    assert_eq!(
        h.storage.get_run(&llm).unwrap().status,
        RunStatus::Completed
    );

    // 5. File patch: hand-write the fix instead.
    let hand_fix = "def init_session(pool, timeout=2.0):\n    conn = pool.acquire(timeout=timeout)\n    return {\"conn\": conn}\n";
    let file = h.fork(&[], "write_fix", "file_content_override", hand_fix);
    assert!(
        h.output(&file, "write_fix")
            .contains("+def init_session(pool, timeout=2.0):")
    );
    assert!(
        h.output(&file, "test_validate")
            .contains("result: OK. 6 passed")
    );
    let changes = h.cli(&["runs", "workspace-diff", &source.0, &file.0]);
    let session = changes
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == "auth/session.py")
        .expect("auth/session.py differs");
    assert_eq!(session["after"], hand_fix);

    // No replay touched the live workspace, and sandboxes were cleaned up.
    assert_eq!(tree_digest(&workspace), live_before);
    assert!(!sandbox_root.exists() || std::fs::read_dir(&sandbox_root).unwrap().next().is_none());
    let _ = std::fs::remove_dir_all(&root);
}
