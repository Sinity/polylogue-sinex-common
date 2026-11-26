use super::*;

fn cand(stratum: &str, rel: &str, size: u64) -> Candidate {
    Candidate {
        path: PathBuf::from(rel),
        rel: PathBuf::from(rel),
        stratum: stratum.into(),
        size,
        group: None,
    }
}

#[test]
fn a_single_file_stratum_is_selected_once_when_the_budget_exceeds_it() {
    let all = vec![cand("synthetic", "one.jsonl", 20)];
    for target in [1, 20, 100] {
        let selected = select(&all, target, 1);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].rel, PathBuf::from("one.jsonl"));
    }
}

fn corpus() -> Vec<Candidate> {
    let mut all = Vec::new();
    for i in 0..1000u64 {
        all.push(cand("a", &format!("a/{i}"), 1_000 + i * 100));
    }
    for i in 0..200u64 {
        all.push(cand("b", &format!("b/{i}"), 50_000 + i * 1_000));
    }
    all.push(cand("b", "b/huge", 2_000_000));
    all.push(cand("c", "c/only", 10));
    all
}

#[test]
fn keeps_each_strata_share_of_bytes() {
    let all = corpus();
    let total: u64 = all.iter().map(|c| c.size).sum();
    let target = total / 10;
    let chosen = select(&all, target, 7);
    let got: u64 = chosen.iter().map(|c| c.size).sum();
    assert!(
        got > target * 8 / 10 && got < target * 12 / 10,
        "{got} vs {target}"
    );
    let share = |items: &[Candidate], s: &str| {
        items
            .iter()
            .filter(|c| c.stratum == s)
            .map(|c| c.size)
            .sum::<u64>() as f64
            / items.iter().map(|c| c.size).sum::<u64>() as f64
    };
    for s in ["a", "b"] {
        assert!(
            (share(&chosen, s) - share(&all, s)).abs() < 0.08,
            "{s}: {} vs {}",
            share(&chosen, s),
            share(&all, s)
        );
    }
}

#[test]
fn small_strata_still_get_a_file_and_the_largest_file_is_kept_when_it_fits() {
    let all = corpus();
    let total: u64 = all.iter().map(|c| c.size).sum();
    let chosen = select(&all, total / 2, 1);
    assert!(chosen.iter().any(|c| c.stratum == "c"));
    assert!(chosen.iter().any(|c| c.rel == Path::new("b/huge")));
}

#[test]
fn the_draw_is_deterministic_and_independent_of_listing_order() {
    let all = corpus();
    let mut reversed = all.clone();
    reversed.reverse();
    let a: Vec<_> = select(&all, 1_000_000, 3)
        .into_iter()
        .map(|c| c.rel)
        .collect();
    let b: Vec<_> = select(&reversed, 1_000_000, 3)
        .into_iter()
        .map(|c| c.rel)
        .collect();
    assert_eq!(a, b);
}

#[test]
fn spread_reports_quantiles() {
    let mut sizes: Vec<u64> = (1..=100).collect();
    let s = Spread::of(&mut sizes);
    assert_eq!((s.files, s.bytes, s.max), (100, 5050, 100));
    assert_eq!(s.p50, 51);
    assert_eq!(s.p99, 99);
}

#[test]
fn materializes_once_and_the_manifest_reads_back_equal() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("x")).unwrap();
    let log = src.join("x/log.jsonl");
    fs::write(&log, "one\ntwo\n").unwrap();
    let doc = src.join("doc.json");
    fs::write(&doc, "{}").unwrap();
    let cands = vec![
        Candidate {
            path: log.clone(),
            rel: "x/log.jsonl".into(),
            stratum: "log".into(),
            size: 8,
            group: Some("x".into()),
        },
        Candidate {
            path: doc,
            rel: "doc.json".into(),
            stratum: "doc".into(),
            size: 2,
            group: None,
        },
    ];
    let m = Manifest::build("p", &cands, &cands, 10, 0).unwrap();
    let dest = dir.path().join("bench/sample");
    assert!(m.materialize(&dest).unwrap());
    assert!(!m.materialize(&dest).unwrap(), "already materialized");
    assert_eq!(
        fs::read_to_string(dest.join("x/log.jsonl")).unwrap(),
        "one\ntwo\n"
    );
    let path = dir.path().join("m.json");
    m.write(&path).unwrap();
    let back = Manifest::read(&path).unwrap();
    assert_eq!(back.entries, m.entries);
    assert_eq!(back.id(), m.id());
    // A source that changed after the draw is refused, not copied.
    fs::write(&log, "one\ntwo\nthree\n").unwrap();
    fs::remove_dir_all(&dest).unwrap();
    assert!(m.materialize(&dest).is_err());
}

#[test]
fn a_drawn_group_includes_every_file_of_it() {
    // Ten sessions of a main file, two subagents and a tool result, each
    // about 1,000 bytes; the target is about three sessions.
    let mut all = Vec::new();
    for s in 0..10u64 {
        for (name, size) in [("main", 600), ("sub-1", 200), ("sub-2", 150), ("tool", 50)] {
            all.push(Candidate {
                path: PathBuf::from(format!("s{s}/{name}")),
                rel: PathBuf::from(format!("s{s}/{name}")),
                stratum: "sessions".into(),
                size: size + s,
                group: Some(format!("s{s}")),
            });
        }
    }
    let chosen = select(&all, 3_000, 5);
    let sessions: std::collections::BTreeSet<_> =
        chosen.iter().map(|c| c.group.clone().unwrap()).collect();
    assert!(!sessions.is_empty() && sessions.len() < 10, "{sessions:?}");
    for session in &sessions {
        let members = chosen
            .iter()
            .filter(|c| c.group.as_ref() == Some(session))
            .count();
        assert_eq!(members, 4, "{session} is drawn whole");
    }
    assert_eq!(chosen.len(), sessions.len() * 4);
}

#[test]
fn never_takes_a_file_bigger_than_its_strata_share() {
    let mut all = corpus();
    all.push(cand("db", "db/huge", 50_000_000));
    all.push(cand("db", "db/small", 1_000));
    let chosen = select(&all, 1_000_000, 2);
    assert!(!chosen.iter().any(|c| c.rel == Path::new("db/huge")));
    assert!(chosen.iter().any(|c| c.stratum == "db"));
}

#[test]
fn incomplete_source_observation_keeps_bench_revision_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Synthetic",
            "-c",
            "user.email=synthetic@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "synthetic",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    fs::write(root.join(".git/index"), b"not a Git index").unwrap();
    let observed = crate::Tree::observe(root);
    assert!(observed.head.is_some());
    assert!(observed.observation_error.is_some());
    let store = Store::open_at(&root.join("runs.db")).unwrap();
    let id = record(
        &store,
        root,
        "demo",
        "build",
        "sample",
        root,
        &json!({"seconds": 1}),
    )
    .unwrap();
    assert!(result(&store, "demo", id).unwrap().unwrap().head.is_none());
}

fn repetition(arm: usize, round: u32, cpu: f64, wall: f64) -> Repetition {
    Repetition {
        arm,
        round,
        // Time first, so the report's ordering is what puts work first.
        figures: vec![
            Figure::time("wall_seconds", Some(wall)),
            Figure::work("cpu_seconds", Some(cpu)),
        ],
        record: json!({}),
    }
}

fn verdict(result: &Value, figure: &str) -> Value {
    result["comparison"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["figure"] == figure)
        .unwrap()
        .clone()
}

fn arms(a: &str, b: &str) -> [Arm; 2] {
    [
        Arm {
            label: a.into(),
            build: json!(a),
        },
        Arm {
            label: b.into(),
            build: json!(b),
        },
    ]
}

#[test]
fn compare_reports_no_difference_when_ranges_overlap() {
    let mut base = [(10.0, 100.0), (10.4, 104.0), (9.8, 99.0)].into_iter();
    let mut same = [(10.2, 98.0), (9.9, 103.0), (10.6, 101.0)].into_iter();
    let repetitions = interleave(2, 3, |arm, round| {
        let (cpu, wall) = if arm == 0 { base.next() } else { same.next() }.unwrap();
        Ok(repetition(arm, round, cpu, wall))
    })
    .unwrap();
    assert_eq!(
        repetitions
            .iter()
            .map(|r| (r.arm, r.round))
            .collect::<Vec<_>>(),
        [(0, 0), (1, 0), (0, 1), (1, 1), (0, 2), (1, 2)],
        "A B A B order"
    );
    let result = report(
        json!({"id": "s"}),
        &Limit::memory("1G"),
        &arms("base", "same"),
        &repetitions,
    );
    let figures: Vec<&str> = result["comparison"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["figure"].as_str().unwrap())
        .collect();
    assert_eq!(figures, ["cpu_seconds", "wall_seconds"], "work before time");
    let cpu = verdict(&result, "cpu_seconds");
    assert_eq!(cpu["verdict"], "no difference");
    assert_eq!(cpu["a"]["median"], 10.0);
    assert_eq!(cpu["b"]["median"], 10.2);
    assert_eq!(verdict(&result, "wall_seconds")["verdict"], "no difference");
    assert_eq!(result["limit"]["memory_max"], "1G");
    assert_eq!(result["repetitions"].as_array().unwrap().len(), 6);
}

#[test]
fn compare_reports_a_ratio_when_ranges_do_not_overlap() {
    let repetitions = vec![
        repetition(0, 0, 10.0, 100.0),
        repetition(1, 0, 15.0, 100.0),
        repetition(0, 1, 11.0, 100.0),
        repetition(1, 1, 16.0, 100.0),
        repetition(0, 2, 12.0, 100.0),
        repetition(1, 2, 18.0, 100.0),
    ];
    let result = report(
        json!({}),
        &Limit::default(),
        &arms("base", "slow"),
        &repetitions,
    );
    let cpu = verdict(&result, "cpu_seconds");
    // Medians 11 and 16; the ratio lies between 15/12 and 18/10.
    assert_eq!(cpu["ratio"], 16.0 / 11.0);
    assert_eq!(cpu["ratio_range"], json!([15.0 / 12.0, 18.0 / 10.0]));
    assert_eq!(cpu["verdict"], "1.455x (1.250 to 1.800)");
    assert_eq!(verdict(&result, "wall_seconds")["verdict"], "no difference");
    assert_eq!(
        result["headline"][0],
        "cpu_seconds (work): 1.455x (1.250 to 1.800)"
    );
}

#[test]
fn an_unknown_reading_leaves_its_figure_unknown() {
    let mut missing = repetition(1, 0, 1.0, 1.0);
    missing.figures[1].value = None;
    let repetitions = vec![repetition(0, 0, 1.0, 1.0), missing];
    let result = report(json!({}), &Limit::default(), &arms("a", "b"), &repetitions);
    assert_eq!(verdict(&result, "cpu_seconds")["verdict"], "unknown");
    assert_eq!(verdict(&result, "cpu_seconds")["b"]["unknown"], 1);
}

#[test]
fn a_result_reads_back_equal() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_at(&dir.path().join("h.db")).unwrap();
    let only = [Arm {
        label: "only".into(),
        build: json!("x"),
    }];
    let repetitions = vec![repetition(0, 0, 2.0, 3.0), repetition(0, 1, 4.0, 5.0)];
    let document = report(json!({"id": "s1"}), &Limit::default(), &only, &repetitions);
    assert_eq!(
        document["summary"][0]["figures"][0]["figure"],
        "cpu_seconds"
    );
    assert_eq!(document["summary"][0]["figures"][0]["median"], 3.0);
    let id = record(
        &store,
        dir.path(),
        "p",
        "build",
        "s1",
        dir.path(),
        &document,
    )
    .unwrap();
    let back = result(&store, "p", id).unwrap().unwrap();
    assert_eq!(back.result, document);
    assert_eq!((back.bench.as_str(), back.sample.as_str()), ("build", "s1"));
    assert_eq!(results(&store, "p", 5).unwrap().len(), 1);
    assert!(result(&store, "other", id).unwrap().is_none());
}

/// The measured child: allocates and touches `DEVKIT_MEASURE_ALLOCATE`
/// bytes, writes and syncs `DEVKIT_MEASURE_WRITE` bytes to
/// `DEVKIT_MEASURE_FILE`, and prints a line.
#[test]
#[ignore = "child of the measurement test, which launches it with its environment"]
fn measured_child() {
    let Some(bytes) = std::env::var_os("DEVKIT_MEASURE_ALLOCATE") else {
        return;
    };
    let bytes: usize = bytes.to_str().unwrap().parse().unwrap();
    let mut memory = vec![0u8; bytes];
    for i in (0..bytes).step_by(4096) {
        memory[i] = 1;
    }
    let written: usize = std::env::var("DEVKIT_MEASURE_WRITE")
        .unwrap()
        .parse()
        .unwrap();
    let path = PathBuf::from(std::env::var_os("DEVKIT_MEASURE_FILE").unwrap());
    let mut file = fs::File::create(&path).unwrap();
    io::Write::write_all(&mut file, &vec![7u8; written]).unwrap();
    file.sync_all().unwrap();
    println!(
        "touched {}",
        memory.iter().map(|b| *b as usize).sum::<usize>()
    );
}

fn child(allocate: usize, file: &Path) -> Process {
    Process::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "bench::tests::measured_child",
            "--ignored",
            "--nocapture",
        ])
        .env("DEVKIT_MEASURE_ALLOCATE", allocate.to_string())
        .env("DEVKIT_MEASURE_WRITE", (8usize << 20).to_string())
        .env("DEVKIT_MEASURE_FILE", file)
}

fn systemd_user_manager() -> bool {
    Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn a_run_under_memory_max_reports_peak_memory_and_io_from_the_unit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("written");
    let measured = measure(&child(64 << 20, &file), &Limit::memory("512M")).unwrap();
    assert!(measured.succeeded(), "{measured:?}");
    assert!(String::from_utf8_lossy(&measured.stdout).contains("touched"));
    assert_eq!(fs::metadata(&file).unwrap().len(), 8 << 20);
    let a = &measured.accounting;
    assert!(a.wall_seconds.is_some_and(|s| s > 0.0), "{a:?}");
    if systemd_user_manager() {
        assert!(a.unit.is_some());
        assert_eq!(a.memory_max.as_deref(), Some("512M"));
        assert!(a.not_applied.is_none());
        assert!(a.memory_peak_bytes.is_some_and(|b| b >= 64 << 20), "{a:?}");
        assert!(a.cpu_seconds.is_some_and(|s| s > 0.0), "{a:?}");
        // Bytes are attributed only where the user manager was delegated the
        // io controller; elsewhere they stay unknown, never zero.
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let io = fs::read_to_string(format!(
            "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/cgroup.subtree_control"
        ))
        .is_ok_and(|c| c.split_whitespace().any(|name| name == "io"));
        assert_eq!(a.read_bytes.is_some(), io, "{a:?}");
        assert_eq!(a.written_bytes.is_some(), io, "{a:?}");
        // Past its limit the same process is killed, and the outcome says so.
        let killed = measure(&child(1 << 30, &file), &Limit::memory("128M")).unwrap();
        assert_eq!(killed.outcome, "oom-kill", "{killed:?}");
        assert!(
            killed
                .accounting
                .memory_peak_bytes
                .is_some_and(|b| b <= 128 << 20),
            "{killed:?}"
        );
    } else {
        // Off systemd the run still happens and says no limit was applied.
        assert!(a.unit.is_none() && a.memory_max.is_none());
        assert!(
            a.not_applied
                .as_deref()
                .is_some_and(|r| r.contains("systemd"))
        );
    }
}
