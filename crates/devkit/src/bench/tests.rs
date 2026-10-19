use super::*;

fn cand(stratum: &str, rel: &str, size: u64) -> Candidate {
    Candidate {
        path: PathBuf::from(rel),
        rel: PathBuf::from(rel),
        stratum: stratum.into(),
        size,
        grows: false,
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
fn materializes_once_cuts_growing_files_and_grows_them_back() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("x")).unwrap();
    let grow_path = src.join("x/log.jsonl");
    fs::write(&grow_path, "one\ntwo\nthree\nfour\n").unwrap();
    let fixed = src.join("doc.json");
    fs::write(&fixed, "{}").unwrap();
    let cands = vec![
        Candidate {
            path: grow_path.clone(),
            rel: "x/log.jsonl".into(),
            stratum: "log".into(),
            size: 19,
            grows: true,
        },
        Candidate {
            path: fixed,
            rel: "doc.json".into(),
            stratum: "doc".into(),
            size: 2,
            grows: false,
        },
    ];
    let m = Manifest::build("p", &cands, &cands, 21, 0, 0.6).unwrap();
    assert_eq!(m.entries[0].cut, Some(8));
    let dest = dir.path().join("bench/sample");
    assert!(m.materialize(&dest).unwrap());
    assert!(!m.materialize(&dest).unwrap(), "already materialized");
    assert_eq!(
        fs::read_to_string(dest.join("x/log.jsonl")).unwrap(),
        "one\ntwo\n"
    );
    assert_eq!(m.grow(&dest).unwrap(), 11);
    assert_eq!(
        fs::read_to_string(dest.join("x/log.jsonl")).unwrap(),
        "one\ntwo\nthree\nfour\n"
    );
    // After growing, the next materialize starts over from the sample.
    assert!(m.materialize(&dest).unwrap());
    let path = dir.path().join("m.json");
    m.write(&path).unwrap();
    let back = Manifest::read(&path).unwrap();
    assert_eq!(back.entries, m.entries);
    assert_eq!(back.id(), m.id());
}

#[test]
fn compares_only_quiet_runs_on_the_same_sample() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_at(&dir.path().join("h.db")).unwrap();
    let quiet = HostLoad {
        load1: 0.1,
        cpus: 16,
        io_full: 0.0,
        memory_full: 0.0,
    };
    let loaded = HostLoad {
        io_full: 60.0,
        ..quiet
    };
    let out = dir.path();
    let a = record(
        &store,
        out,
        "p",
        "build",
        "s1",
        out,
        quiet,
        &json!({"seconds": 100.0}),
    )
    .unwrap();
    let b = record(
        &store,
        out,
        "p",
        "build",
        "s1",
        out,
        quiet,
        &json!({"seconds": 80.0}),
    )
    .unwrap();
    let c = record(
        &store,
        out,
        "p",
        "build",
        "s1",
        out,
        loaded,
        &json!({"seconds": 300.0}),
    )
    .unwrap();
    let cmp = compare(&store, b).unwrap();
    assert_eq!(cmp["against"], json!(a));
    assert!((cmp["changes"]["seconds"]["change"].as_f64().unwrap() + 0.2).abs() < 1e-9);
    assert_eq!(compare(&store, c).unwrap()["reliable"], json!(false));
}

#[test]
fn predicts_linearly_per_stage() {
    let stages = BTreeMap::from([("parse".to_owned(), 10.0)]);
    let p = predict(60.0, &stages, 20.0);
    assert_eq!(p["seconds"], json!(1200.0));
    assert_eq!(p["stages"]["parse"], json!(200.0));
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
