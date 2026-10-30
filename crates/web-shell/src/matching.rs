//! One subsequence matcher for the web palette and terminal pickers. Unicode
//! lowercasing precedes matching; adjacent letters and word starts score higher.
pub fn match_score(query: &str, label: &str) -> Option<f64> {
    let needle: Vec<_> = query.to_lowercase().chars().collect();
    let haystack: Vec<_> = label.to_lowercase().chars().collect();
    let mut next = 0;
    let mut previous = None;
    let mut score = 0.0;
    for character in &needle {
        let found = haystack[next..]
            .iter()
            .position(|value| value == character)?
            + next;
        score += if previous.is_some_and(|previous| previous + 1 == found) {
            8.0
        } else {
            1.0
        };
        if found == 0
            || matches!(
                haystack[found - 1],
                ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}' | '_' | '/' | '-'
            )
        {
            score += 4.0;
        }
        previous = Some(found);
        next = found + 1;
    }
    Some(score - (haystack.len() as f64 - needle.len() as f64) / 1000.0)
}
