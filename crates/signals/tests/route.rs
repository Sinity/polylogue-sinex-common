//! The route function's table and properties over every combination of inputs.
use signals::*;

const MINUTE: i64 = 60_000;
const NOW: i64 = 1_000 * MINUTE;

fn notice() -> Candidate {
    Candidate {
        interrupt: true,
        urgency: Urgency::Normal,
        author: AuthorClass::Operator,
        subjects: vec!["session:a".into()],
        quiet_hours_until: None,
        inhibited_by: None,
        deferred_since: None,
    }
}
fn attention() -> Attention {
    Attention {
        at: NOW,
        budget_per_hour: 4,
        ..Attention::default()
    }
}
fn passive(reason: PassiveReason) -> Decision {
    Decision::Passive { reason }
}
fn hold(until: Option<i64>, reason: HoldReason) -> Decision {
    Decision::Hold { until, reason }
}
const SHOW: Decision = Decision::Show { send_away: false };

#[test]
fn route_applies_its_rules_in_order() {
    let quiet = Some(NOW + 60 * MINUTE);
    let cases: Vec<(&str, Candidate, Attention, Decision)> = vec![
        (
            "passive rule",
            Candidate {
                interrupt: false,
                inhibited_by: Some("root".into()),
                ..notice()
            },
            attention(),
            passive(PassiveReason::RuleIsPassive),
        ),
        (
            "inhibited before critical",
            Candidate {
                inhibited_by: Some("root".into()),
                urgency: Urgency::Critical,
                ..notice()
            },
            attention(),
            passive(PassiveReason::CausedBy("root".into())),
        ),
        (
            "critical in quiet hours while away",
            Candidate {
                urgency: Urgency::Critical,
                quiet_hours_until: quiet,
                author: AuthorClass::Agent,
                ..notice()
            },
            Attention {
                presence: Presence::Away,
                away_command: true,
                budget_used: 9,
                ..attention()
            },
            Decision::Show { send_away: true },
        ),
        (
            "agent past budget, before quiet",
            Candidate {
                author: AuthorClass::Agent,
                quiet_hours_until: quiet,
                ..notice()
            },
            Attention {
                budget_used: 4,
                ..attention()
            },
            passive(PassiveReason::BudgetSpent),
        ),
        (
            "operator past budget is held by quiet",
            Candidate {
                quiet_hours_until: quiet,
                ..notice()
            },
            Attention {
                budget_used: 4,
                ..attention()
            },
            hold(quiet, HoldReason::Quiet),
        ),
        (
            "explicit quiet, the later end wins",
            Candidate {
                quiet_hours_until: quiet,
                ..notice()
            },
            Attention {
                quiet_until: Some(NOW + 90 * MINUTE),
                ..attention()
            },
            hold(Some(NOW + 90 * MINUTE), HoldReason::Quiet),
        ),
        (
            "an ended explicit quiet does not hold",
            notice(),
            Attention {
                quiet_until: Some(NOW),
                ..attention()
            },
            SHOW,
        ),
        (
            "on screen before away",
            notice(),
            Attention {
                focus: vec!["session:a".into()],
                presence: Presence::Away,
                ..attention()
            },
            passive(PassiveReason::OnScreen),
        ),
        (
            "away with a route",
            notice(),
            Attention {
                presence: Presence::Away,
                away_command: true,
                ..attention()
            },
            Decision::SendAway,
        ),
        (
            "away without a route",
            notice(),
            Attention {
                presence: Presence::Away,
                ..attention()
            },
            hold(None, HoldReason::Away),
        ),
        (
            "deep work holds agents by default",
            Candidate {
                author: AuthorClass::Agent,
                ..notice()
            },
            Attention {
                deep_work: true,
                ..attention()
            },
            hold(Some(NOW + BREAKPOINT_BOUND), HoldReason::DeepWork),
        ),
        (
            "deep work spares the operator by default",
            notice(),
            Attention {
                deep_work: true,
                ..attention()
            },
            SHOW,
        ),
        (
            "deep work holds everyone when configured",
            notice(),
            Attention {
                deep_work: true,
                deep_work_deferral: DeepWorkDeferral::All,
                ..attention()
            },
            hold(Some(NOW + BREAKPOINT_BOUND), HoldReason::DeepWork),
        ),
        (
            "a breakpoint releases",
            Candidate {
                author: AuthorClass::Producer,
                deferred_since: Some(NOW - MINUTE),
                ..notice()
            },
            Attention {
                deep_work: true,
                breakpoint: true,
                ..attention()
            },
            SHOW,
        ),
        (
            "the bound releases",
            Candidate {
                author: AuthorClass::Agent,
                deferred_since: Some(NOW - BREAKPOINT_BOUND),
                ..notice()
            },
            Attention {
                deep_work: true,
                ..attention()
            },
            SHOW,
        ),
        (
            "unknown presence is never away",
            notice(),
            Attention {
                presence: Presence::Unknown,
                ..attention()
            },
            SHOW,
        ),
    ];
    for (name, candidate, attention, expected) in cases {
        assert_eq!(route(&candidate, &attention), expected, "{name}");
    }
}

/// Every combination of the inputs the rule depends on.
fn candidates() -> Vec<Candidate> {
    let mut all = Vec::new();
    for interrupt in [false, true] {
        for urgency in [Urgency::Normal, Urgency::Critical] {
            for author in [
                AuthorClass::Operator,
                AuthorClass::Agent,
                AuthorClass::Producer,
            ] {
                for quiet in [None, Some(NOW + MINUTE)] {
                    for inhibited in [None, Some("root".to_string())] {
                        all.push(Candidate {
                            interrupt,
                            urgency,
                            author,
                            subjects: vec!["session:a".into()],
                            quiet_hours_until: quiet,
                            inhibited_by: inhibited,
                            deferred_since: None,
                        });
                    }
                }
            }
        }
    }
    all
}
fn attention_states(budget_used: usize) -> Vec<Attention> {
    let mut all = Vec::new();
    for presence in [Presence::Present, Presence::Away, Presence::Unknown] {
        for focus in [vec![], vec!["session:a".to_string()]] {
            for deep_work in [false, true] {
                for away_command in [false, true] {
                    all.push(Attention {
                        presence,
                        focus: focus.clone(),
                        deep_work,
                        away_command,
                        budget_used,
                        ..attention()
                    });
                }
            }
        }
    }
    all
}

#[test]
fn sig_route_pure() {
    for candidate in candidates() {
        for attention in attention_states(2) {
            let decision = route(&candidate, &attention);
            let copied: (Candidate, Attention) =
                serde_json::from_value(serde_json::to_value((&candidate, &attention)).unwrap())
                    .unwrap();
            assert_eq!(route(&copied.0, &copied.1), decision);
        }
    }
}

#[test]
fn sig_attention_only_removes() {
    for budget_used in [0, 4] {
        for candidate in candidates() {
            let unknown = Attention {
                budget_used,
                ..attention()
            };
            let baseline = route(&candidate, &unknown);
            // With every attention input unknown, routing is v3's: passive,
            // inhibited, critical, budget, quiet hours, else shown.
            let v3 = if !candidate.interrupt {
                passive(PassiveReason::RuleIsPassive)
            } else if let Some(root) = &candidate.inhibited_by {
                passive(PassiveReason::CausedBy(root.clone()))
            } else if candidate.urgency == Urgency::Critical {
                SHOW
            } else if candidate.author != AuthorClass::Operator && budget_used >= 4 {
                passive(PassiveReason::BudgetSpent)
            } else if let Some(until) = candidate.quiet_hours_until {
                hold(Some(until), HoldReason::Quiet)
            } else {
                SHOW
            };
            assert_eq!(baseline, v3, "{candidate:?}");
            for informed in attention_states(budget_used) {
                let decision = route(&candidate, &informed);
                if !baseline.interrupts() {
                    assert!(!decision.interrupts(), "{candidate:?} {informed:?}");
                }
            }
        }
    }
}

#[test]
fn sig_critical_never_deferred() {
    for candidate in candidates().into_iter().filter(|candidate| {
        candidate.interrupt
            && candidate.urgency == Urgency::Critical
            && candidate.inhibited_by.is_none()
    }) {
        for attention in attention_states(9).into_iter().chain([Attention {
            quiet_until: Some(NOW + MINUTE),
            ..attention()
        }]) {
            let decision = route(&candidate, &attention);
            assert!(
                matches!(decision, Decision::Show { .. }),
                "{candidate:?} {attention:?} {decision:?}"
            );
        }
    }
}
