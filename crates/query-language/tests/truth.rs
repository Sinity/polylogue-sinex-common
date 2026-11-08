use query_language::semantics::Truth;

#[test]
fn truth_tables_cover_every_pair_and_negation() {
    use Truth::{False as F, True as T, Unknown as U};
    for (a, b, and, or) in [
        (F, F, F, F),
        (F, U, F, U),
        (F, T, F, T),
        (U, F, F, U),
        (U, U, U, U),
        (U, T, U, T),
        (T, F, F, T),
        (T, U, U, T),
        (T, T, T, T),
    ] {
        assert_eq!(a.and(b), and);
        assert_eq!(a.or(b), or);
    }
    for (a, expected) in [(T, F), (F, T), (U, U)] {
        assert_eq!(!a, expected);
        assert_eq!(a.not(), expected);
        assert_eq!(a.matches(), a == T);
    }
}
