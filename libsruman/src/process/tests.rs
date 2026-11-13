use std::{
    os::unix::process::CommandExt,
    process::{Child, Command},
};

use super::*;

fn fake_process(
    root: &Path,
    pid: i32,
    ppid: i32,
    pgid: i32,
    start: u64,
    env: &[(&str, &str)],
    comm: &str,
) {
    let dir = root.join(pid.to_string());
    fs::create_dir_all(&dir).unwrap();
    let environ: Vec<u8> = env
        .iter()
        .flat_map(|(k, v)| format!("{k}={v}\0").into_bytes())
        .collect();
    fs::write(dir.join("environ"), environ).unwrap();
    fs::write(
        dir.join("stat"),
        format!(
            "{pid} ({comm}) S {ppid} {pgid} {pgid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {start} 0 0"
        ),
    )
    .unwrap();
    fs::write(dir.join("cmdline"), format!("{comm}\0--flag\0")).unwrap();
}

#[test]
fn stat_with_awkward_comm() {
    let line = "42 (a) b (c)) S 7 42 42 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 12345 0 0";
    assert_eq!(parse_stat(line), Some((7, 42, 12345)));
    assert_eq!(parse_stat("garbage"), None);
    let env = parse_environ(b"A=1\0B=x=y\0broken\0\0");
    assert_eq!(env.len(), 2);
    assert_eq!(env["B"], "x=y");
}

#[test]
fn sessions_are_grouped_by_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let game = [
        (TAG_GAME, "supertux"),
        (TAG_PROFILE, "sniper"),
        (TAG_PREFIX, "/p/indie"),
    ];
    fake_process(root, 100, 1, 100, 500, &game, "run");
    fake_process(
        root,
        101,
        100,
        100,
        510,
        &[(TAG_GAME, "supertux")],
        "pv-adverb (x)",
    );
    fake_process(
        root,
        102,
        101,
        100,
        520,
        &[(TAG_GAME, "supertux")],
        "supertux2",
    );
    fake_process(
        root,
        200,
        1,
        200,
        300,
        &[(TAG_SESSION, "exec:sniper:9"), (TAG_PROFILE, "sniper")],
        "run",
    );
    fake_process(root, 300, 1, 300, 100, &[("PATH", "/bin")], "bash");
    fs::create_dir_all(root.join("400")).unwrap(); // unreadable: no files
    fs::create_dir_all(root.join("self")).unwrap();

    let sessions = list_in(root);
    assert_eq!(sessions.len(), 2);
    let game = &sessions[0];
    assert_eq!(game.tag, SessionTag::Game("supertux".into()));
    assert_eq!(game.tag.to_string(), "supertux");
    assert_eq!(game.profile.as_deref(), Some("sniper"));
    assert_eq!(game.prefix.as_deref(), Some(Path::new("/p/indie")));
    assert_eq!(game.leader.pid, 100);
    assert_eq!(game.leader.cmdline, ["run", "--flag"]);
    assert_eq!(
        game.processes.iter().map(|p| p.pid).collect::<Vec<_>>(),
        [100, 101, 102]
    );
    let exec = &sessions[1];
    assert_eq!(exec.tag, SessionTag::Session("exec:sniper:9".into()));
    assert_eq!(exec.prefix, None);

    assert!(list_in(Path::new("/nonexistent")).is_empty());
}

fn unique(name: &str) -> String {
    format!("sruman-test-{name}-{}", std::process::id())
}

/// Spawns `sh -c script` tagged as game `tag`, in its own session.
fn spawn_tagged(tag: &str, script: &str) -> Child {
    let mut command = Command::new("sh");
    command
        .args(["-c", script])
        .env(TAG_GAME, tag)
        .env(TAG_PROFILE, "test");
    command.process_group(0);
    command.spawn().unwrap()
}

fn wait_for(tag: &str, count: usize) -> Session {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(session) = find_game(tag)
            && session.processes.len() >= count
        {
            return session;
        }
        assert!(Instant::now() < deadline, "session {tag} never appeared");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn stop_terminates_a_session() {
    let _spawn = crate::test_lock::spawns();
    let tag = unique("term");
    let mut child = spawn_tagged(&tag, "sleep 30 & wait");
    let session = wait_for(&tag, 2);
    assert_eq!(session.leader.pid, child.id() as i32);
    assert_eq!(session.profile.as_deref(), Some("test"));

    assert_eq!(
        stop(&session, Duration::from_secs(5)).unwrap(),
        Stopped::Terminated
    );
    child.wait().unwrap();
    assert!(find_game(&tag).is_none());
    // Stopping again is fine.
    assert_eq!(
        stop(&session, Duration::from_secs(1)).unwrap(),
        Stopped::Terminated
    );
}

#[test]
fn stop_kills_what_ignores_term() {
    let _spawn = crate::test_lock::spawns();
    let tag = unique("kill");
    let mut child = spawn_tagged(&tag, "trap '' TERM; sleep 30 & wait; sleep 30");
    let session = wait_for(&tag, 2);
    let started = Instant::now();
    assert_eq!(
        stop(&session, Duration::from_millis(300)).unwrap(),
        Stopped::Killed
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    child.wait().unwrap();
    assert!(find_game(&tag).is_none());
}

#[test]
fn stop_reaches_tagged_processes_outside_the_group() {
    let _spawn = crate::test_lock::spawns();
    if Command::new("setsid").arg("--version").output().is_err() {
        return;
    }
    let tag = unique("escape");
    // The inner sleep moves to a session of its own, out of the group.
    let mut child = spawn_tagged(&tag, "setsid sleep 30 & sleep 30 & wait");
    let session = wait_for(&tag, 3);
    let groups: std::collections::BTreeSet<i32> =
        session.processes.iter().map(|p| p.pgid).collect();
    assert!(groups.len() >= 2, "{session:#?}");
    assert_eq!(
        stop(&session, Duration::from_secs(5)).unwrap(),
        Stopped::Terminated
    );
    child.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while find_game(&tag).is_some() {
        assert!(Instant::now() < deadline, "a straggler survived");
        thread::sleep(Duration::from_millis(20));
    }
}
