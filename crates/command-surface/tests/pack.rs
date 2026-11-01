//! The budget fitter and section status.
use command_surface::*;
use proptest::prelude::*;
use serde_json::json;

fn left_out_line(item: &LeftOut) -> String {
    format!("left out: {} of {}\n", item.count, item.what)
}
fn section(title: &str, pending: bool, entries: usize) -> Section {
    Section {
        title: title.into(),
        status: SectionStatus::Complete,
        pending,
        entries: (0..entries)
            .map(|n| Entry::new(format!("{title} entry {n}"), format!("ref:{title}/{n}")))
            .collect(),
        expand: Some(RunnableRequest::new(
            "read",
            json!({ "section": title }),
            "x",
        )),
    }
}
fn words(text: &str) -> u32 {
    u32::try_from(text.split_whitespace().count()).unwrap()
}

#[test]
fn pending_first() {
    // Room for one section and a left-out line: the pending one wins
    // although it comes second. Each heading is 2 words, each entry 3, each
    // left-out line 5; the waiting section takes 11 of 16.
    let sections = vec![section("past", false, 3), section("waiting", true, 3)];
    let fitted = fit(
        sections,
        Budget::new(Some(16), 0, None),
        &words,
        &left_out_line,
    );
    let titles: Vec<&str> = fitted.sections.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["waiting"]);
    assert_eq!(fitted.sections[0].entries.len(), 3);
    assert_eq!(fitted.left_out.len(), 1);
    assert_eq!(fitted.left_out[0].what, "past");
    assert_eq!(fitted.left_out[0].count, 3);
    assert_eq!(
        fitted.left_out[0].expand.as_ref().unwrap().arguments,
        json!({"section": "past"})
    );
}

#[test]
fn whole_entries() {
    let mut multi = section("rows", false, 0);
    multi.entries = vec![
        Entry::new("row zero", "ref:0"),
        Entry::with_lines(
            vec!["row one".into(), "  detail of row one".into()],
            vec!["ref:1".into()],
        )
        .unwrap(),
    ];
    // The heading (2 words), row zero (2) and a left-out line (5) fit in 9;
    // the two-line entry (6) does not, and none of its lines is kept.
    let fitted = fit(
        vec![multi],
        Budget::new(Some(9), 0, None),
        &words,
        &left_out_line,
    );
    assert_eq!(fitted.text, "## rows\nrow zero\n");
    assert_eq!(fitted.left_out[0].count, 1);
    assert_eq!(fitted.used_tokens, 9);
}

#[test]
fn ample_budget_leaves_nothing_out() {
    let sections: Vec<Section> = (0..40)
        .map(|n| section(&format!("s{n}"), n % 3 == 0, 2))
        .collect();
    let fitted = fit(
        sections,
        Budget::new(None, 100_000, None),
        &words,
        &left_out_line,
    );
    assert!(fitted.left_out.is_empty());
    assert_eq!(fitted.sections.len(), 40);
    // The text keeps the given order, not the fill order.
    assert!(fitted.text.starts_with("## s0\n"));
    assert!(fitted.text.find("## s1\n").unwrap() < fitted.text.find("## s3\n").unwrap());
}

#[test]
fn budget_clamped_to_the_callers_cap_says_so() {
    let budget = Budget::new(Some(50_000), 4_000, Some(25_000));
    assert_eq!(budget.tokens, 25_000);
    assert_eq!(budget.clamped_from, Some(50_000));
    assert!(budget.note().unwrap().contains("50000 to 25000"));
    assert_eq!(Budget::new(None, 4_000, Some(25_000)).note(), None);
    assert_eq!(approximate_tokens("abcde"), 2);
}

#[test]
fn every_entry_cited() {
    assert!(Entry::with_lines(vec!["text".into()], vec![]).is_none());
    assert!(Entry::with_lines(vec![], vec!["ref:1".into()]).is_none());
    let uncited = json!({"lines": ["text"], "cites": []});
    assert!(serde_json::from_value::<Entry>(uncited).is_err());
    let entry = Entry::new("text", "ref:1").cite("ref:2");
    assert_eq!(entry.cites(), ["ref:1", "ref:2"]);
    let wire = serde_json::to_value(&entry).unwrap();
    assert_eq!(serde_json::from_value::<Entry>(wire).unwrap(), entry);
}

#[test]
fn section_status_travels_with_its_reason() {
    for status in [
        SectionStatus::Complete,
        SectionStatus::Partial {
            reason: "one source behind".into(),
        },
        SectionStatus::NotEstimable {
            reason: "no denominator".into(),
        },
        SectionStatus::NotObserved {
            sources: vec!["shell".into()],
        },
        SectionStatus::Empty,
    ] {
        let mut value = section("s", false, 1);
        value.status = status;
        let wire = serde_json::to_value(&value).unwrap();
        assert_eq!(serde_json::from_value::<Section>(wire).unwrap(), value);
    }
    let mut empty = section("Open loops", false, 0);
    empty.status = SectionStatus::Empty;
    let fitted = fit(
        vec![empty],
        Budget::new(Some(100), 0, None),
        &words,
        &left_out_line,
    );
    assert_eq!(fitted.text, "## Open loops (none)\n");
}

proptest! {
    #[test]
    fn fit_within_budget(
        shape in proptest::collection::vec((any::<bool>(), 0usize..6), 0..8),
        budget in 0u32..200,
    ) {
        let sections: Vec<Section> = shape
            .iter()
            .enumerate()
            .map(|(n, (pending, entries))| section(&format!("section{n}"), *pending, *entries))
            .collect();
        let total: usize = shape.iter().map(|(_, entries)| entries).sum();
        let reserve: u32 = sections
            .iter()
            .map(|s| words(&left_out_line(&LeftOut { what: s.title.clone(), count: s.entries.len() as u64, expand: None })))
            .sum();
        let fitted = fit(sections.clone(), Budget::new(Some(budget), 0, None), &words, &left_out_line);
        // What was kept and what was left out add up to everything.
        let kept: usize = fitted.sections.iter().map(|s| s.entries.len()).sum();
        let left: u64 = fitted.left_out.iter().map(|l| l.count).sum();
        prop_assert_eq!(kept as u64 + left, total as u64);
        // Kept entries are a prefix of each section's, whole.
        for kept in &fitted.sections {
            let original = sections.iter().find(|s| s.title == kept.title).unwrap();
            prop_assert_eq!(&original.entries[..kept.entries.len()], kept.entries.as_slice());
        }
        if reserve <= budget {
            prop_assert!(fitted.used_tokens <= budget, "{} > {}", fitted.used_tokens, budget);
        }
        let measured = words(&fitted.text)
            + fitted.left_out.iter().map(|l| words(&left_out_line(l))).sum::<u32>();
        prop_assert_eq!(fitted.used_tokens, measured);
    }
}
