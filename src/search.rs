//! Matching and ranking of app names against what the user typed.
//!
//! Tiers, best first:
//!
//! 1. exact name
//! 2. name prefix
//! 3. word prefix
//! 4. initials ("vsc" -> Visual Studio Code)
//! 5. substring
//! 6. fuzzy subsequence
//!
//! Within a tier, shorter names win, then alphabetical order.

/// A name prepared for matching: lowercased chars plus the indices where words start.
#[derive(Clone)]
pub struct Searchable {
    pub lower: Vec<char>,
    pub word_starts: Vec<usize>,
}

impl Searchable {
    pub fn new(name: &str) -> Self {
        let chars: Vec<char> = name.chars().collect();
        // One lowercase char per original char keeps indices aligned.
        let lower: Vec<char> = chars
            .iter()
            .map(|c| c.to_lowercase().next().unwrap_or(*c))
            .collect();
        let mut word_starts = Vec::new();
        for i in 0..chars.len() {
            let c = chars[i];
            if !c.is_alphanumeric() {
                continue;
            }
            let start = match i.checked_sub(1).map(|p| chars[p]) {
                None => true,
                Some(prev) if !prev.is_alphanumeric() => true,
                // camelCase / PascalCase boundary: "OneNote" -> One, Note
                Some(prev) if prev.is_lowercase() && c.is_uppercase() => true,
                // letter/digit boundary: "Office365" -> Office, 365
                Some(prev) if prev.is_alphabetic() != c.is_alphabetic() => true,
                _ => false,
            };
            if start {
                word_starts.push(i);
            }
        }
        Searchable { lower, word_starts }
    }
}

/// Lowercased query chars, split into whitespace-separated tokens.
pub struct Query {
    full: Vec<char>,
    tokens: Vec<Vec<char>>,
}

impl Query {
    pub fn new(text: &str) -> Self {
        let lower: String = strip_command_prefix(text).to_lowercase();
        Query {
            full: lower.chars().collect(),
            tokens: lower
                .split_whitespace()
                .map(|t| t.chars().collect())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.full.is_empty()
    }
}

/// Trims the text and drops a leading "/" (command mode, see `is_command_query`), so
/// "/update" is searched as "update".
pub fn strip_command_prefix(text: &str) -> &str {
    text.trim().trim_start_matches('/').trim_start()
}

/// Command mode: the text starts with "/". Only commands and settings are searched
/// then, and "/" alone lists all of them.
pub fn is_command_query(text: &str) -> bool {
    text.trim_start().starts_with('/')
}

/// File search mode: the text starts with "%" (see filesearch.rs). A calculation
/// never starts with "%", so this does not clash with the calculator's percent.
pub fn is_file_query(text: &str) -> bool {
    text.trim_start().starts_with('%')
}

/// The text after the leading "%", trimmed.
pub fn strip_file_prefix(text: &str) -> &str {
    text.trim_start().trim_start_matches('%').trim()
}

/// Custom command mode: the text starts with "?" (see custom.rs).
pub fn is_custom_query(text: &str) -> bool {
    text.trim_start().starts_with('?')
}

/// The text after the leading "?", trimmed at the start.
pub fn strip_custom_prefix(text: &str) -> &str {
    text.trim_start().trim_start_matches('?').trim_start()
}

fn starts_with_at(hay: &[char], at: usize, needle: &[char]) -> bool {
    hay.len() >= at + needle.len() && hay[at..at + needle.len()] == *needle
}

fn find(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| starts_with_at(hay, i, needle))
}

/// Score of one token against a name, or None if it does not match at all.
fn score_token(q: &[char], s: &Searchable) -> Option<i32> {
    let name = &s.lower;
    if q.is_empty() {
        return None;
    }
    if *name == *q {
        return Some(1000);
    }
    if starts_with_at(name, 0, q) {
        return Some(900);
    }
    if s.word_starts.iter().any(|&w| starts_with_at(name, w, q)) {
        return Some(800);
    }
    if q.len() >= 2 {
        let initials: Vec<char> = s.word_starts.iter().map(|&w| name[w]).collect();
        if starts_with_at(&initials, 0, q) {
            return Some(750);
        }
    }
    if let Some(pos) = find(name, q) {
        return Some(600 - pos.min(50) as i32);
    }
    fuzzy(q, s)
}

/// Subsequence match ("vscde" -> "Visual Studio Code"), rewarding word starts and runs.
fn fuzzy(q: &[char], s: &Searchable) -> Option<i32> {
    let name = &s.lower;
    let mut score = 300;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, &c) in name.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            if s.word_starts.contains(&i) {
                score += 15;
            }
            match last {
                Some(l) if l + 1 == i => score += 10,
                Some(l) => score -= ((i - l - 1) as i32).min(10),
                None => score -= (i as i32).min(10),
            }
            last = Some(i);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(score.clamp(100, 499))
}

/// Score of a whole query against a name. Multi-word queries need every token to match.
pub fn score(query: &Query, s: &Searchable) -> Option<i32> {
    if query.is_empty() {
        return None;
    }
    if query.tokens.len() <= 1 {
        return score_token(&query.full, s);
    }
    if let Some(sc) = score_token(&query.full, s) {
        if sc >= 600 {
            return Some(sc); // the query matches as typed, spaces included
        }
    }
    let mut total = 0;
    for t in &query.tokens {
        total += score_token(t, s)?;
    }
    Some(total / query.tokens.len() as i32 - 50)
}

/// Indices of the best `limit` matches, best first, by text match only.
#[cfg(test)]
pub fn rank<'a, I>(query: &Query, items: I, limit: usize) -> Vec<usize>
where
    I: IntoIterator<Item = (&'a str, &'a Searchable)>,
{
    rank_with(query, items, limit, |_| 0)
}

/// Like `rank`, with an extra bonus per item (e.g. from launch history) added to
/// the text score. Only items that match the text at all can appear.
#[cfg(test)]
pub fn rank_with<'a, I>(
    query: &Query,
    items: I,
    limit: usize,
    boost: impl Fn(usize) -> i32,
) -> Vec<usize>
where
    I: IntoIterator<Item = (&'a str, &'a Searchable)>,
{
    let no_aliases: &[Searchable] = &[];
    rank_items(
        query,
        items.into_iter().map(|(name, s)| (name, s, no_aliases)),
        limit,
        boost,
    )
}

/// Where Ctrl+Backspace should delete back to, for UTF-16 `text` and caret position
/// `caret`: skips spaces, then one run of letters/digits (or of other symbols),
/// like Windows Explorer and browsers do.
pub fn word_start_before(text: &[u16], caret: usize) -> usize {
    let chars: Vec<(usize, char)> = char::decode_utf16(text.iter().copied())
        .scan(0usize, |pos, c| {
            let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
            let start = *pos;
            *pos += c.len_utf16();
            Some((start, c))
        })
        .take_while(|(start, _)| *start < caret)
        .collect();
    let mut i = chars.len();
    while i > 0 && chars[i - 1].1.is_whitespace() {
        i -= 1;
    }
    if i > 0 {
        let word = chars[i - 1].1.is_alphanumeric();
        while i > 0 && !chars[i - 1].1.is_whitespace() && chars[i - 1].1.is_alphanumeric() == word {
            i -= 1;
        }
    }
    chars
        .get(i)
        .map_or(caret.min(text.len()), |(start, _)| *start)
}

/// A match through an alias ranks slightly below the same match on the shown name.
const ALIAS_PENALTY: i32 = 20;

/// Full ranking: items are (shown name, its searchable form, aliases). An item matches
/// if its name or any alias matches; the best of those scores counts, plus `boost`.
pub fn rank_items<'a, I>(
    query: &Query,
    items: I,
    limit: usize,
    boost: impl Fn(usize) -> i32,
) -> Vec<usize>
where
    I: IntoIterator<Item = (&'a str, &'a Searchable, &'a [Searchable])>,
{
    let mut hits: Vec<(i32, usize, &str, usize)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(i, (name, s, aliases))| {
            let by_alias = aliases
                .iter()
                .filter_map(|a| score(query, a))
                .max()
                .map(|sc| sc - ALIAS_PENALTY);
            let best = score(query, s).into_iter().chain(by_alias).max()?;
            Some((best + boost(i), s.lower.len(), name, i))
        })
        .collect();
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(a.1.cmp(&b.1))
            .then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase()))
    });
    hits.into_iter().take(limit).map(|h| h.3).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(query: &str, names: &[&str]) -> Vec<String> {
        let prepared: Vec<Searchable> = names.iter().map(|n| Searchable::new(n)).collect();
        let q = Query::new(query);
        rank(&q, names.iter().copied().zip(prepared.iter()), 10)
            .into_iter()
            .map(|i| names[i].to_string())
            .collect()
    }

    const APPS: &[&str] = &[
        "Visual Studio Code",
        "Visual Studio Installer",
        "Rechner",
        "Windows PowerShell",
        "PowerToys",
        "Microsoft Edge",
        "OneNote",
        "Notepad++",
        "Editor",
        "Code Writer",
    ];

    #[test]
    fn word_starts_cover_spaces_camel_case_and_digits() {
        assert_eq!(
            Searchable::new("Visual Studio Code").word_starts,
            vec![0, 7, 14]
        );
        assert_eq!(Searchable::new("OneNote").word_starts, vec![0, 3]);
        assert_eq!(Searchable::new("Office365").word_starts, vec![0, 6]);
        assert_eq!(Searchable::new("Notepad++").word_starts, vec![0]);
    }

    #[test]
    fn empty_query_matches_nothing() {
        assert!(top("", APPS).is_empty());
        assert!(top("   ", APPS).is_empty());
    }

    #[test]
    fn prefix_beats_word_prefix() {
        let r = top("code", APPS);
        assert_eq!(r[0], "Code Writer");
        assert_eq!(r[1], "Visual Studio Code");
    }

    #[test]
    fn initials_find_visual_studio_code() {
        assert_eq!(top("vsc", APPS)[0], "Visual Studio Code");
        assert_eq!(top("vsi", APPS)[0], "Visual Studio Installer");
    }

    #[test]
    fn word_prefix_in_middle_of_name() {
        assert_eq!(top("power", APPS)[0], "PowerToys");
        assert!(top("power", APPS).contains(&"Windows PowerShell".to_string()));
        assert_eq!(top("note", APPS)[0], "Notepad++");
        assert!(top("note", APPS).contains(&"OneNote".to_string()));
    }

    #[test]
    fn case_insensitive_and_umlauts() {
        assert_eq!(top("RECH", APPS)[0], "Rechner");
        assert_eq!(top("über", &["Über Windows", "Uhr"])[0], "Über Windows");
    }

    #[test]
    fn multi_word_query_needs_all_tokens() {
        assert_eq!(top("stu code", APPS), vec!["Visual Studio Code"]);
        assert!(top("visual xyz", APPS).is_empty());
    }

    #[test]
    fn fuzzy_subsequence_as_last_resort() {
        assert_eq!(top("vscde", APPS)[0], "Visual Studio Code");
        assert!(top("zzz", APPS).is_empty());
    }

    #[test]
    fn boost_reorders_but_never_adds_non_matches() {
        let prepared: Vec<Searchable> = APPS.iter().map(|n| Searchable::new(n)).collect();
        let items = || APPS.iter().copied().zip(prepared.iter());
        let one_note = APPS.iter().position(|n| *n == "OneNote").unwrap();
        let rechner = APPS.iter().position(|n| *n == "Rechner").unwrap();
        let boost = |i: usize| {
            if i == one_note || i == rechner {
                400
            } else {
                0
            }
        };
        let r = rank_with(&Query::new("note"), items(), 10, boost);
        assert_eq!(APPS[r[0]], "OneNote");
        assert!(
            !r.contains(&rechner),
            "a boost must not pull in non-matching apps"
        );
    }

    #[test]
    fn alias_finds_item_but_ranks_below_name_match() {
        let names = ["Herunterfahren", "Shutdown Timer"];
        let prepared: Vec<Searchable> = names.iter().map(|n| Searchable::new(n)).collect();
        let aliases = [vec![Searchable::new("Shut down")], Vec::new()];
        let search = |q: &str| -> Vec<&str> {
            let items = (0..names.len()).map(|i| (names[i], &prepared[i], aliases[i].as_slice()));
            rank_items(&Query::new(q), items, 10, |_| 0)
                .into_iter()
                .map(|i| names[i])
                .collect()
        };
        assert_eq!(search("shut"), vec!["Shutdown Timer", "Herunterfahren"]);
        assert_eq!(search("herunter"), vec!["Herunterfahren"]);
        assert!(search("xyz").is_empty());
    }

    #[test]
    fn ctrl_backspace_word_boundaries() {
        let w = |s: &str, caret: usize| {
            let text: Vec<u16> = s.encode_utf16().collect();
            let start = word_start_before(&text, caret);
            String::from_utf16_lossy(&text[..start]) + &String::from_utf16_lossy(&text[caret..])
        };
        assert_eq!(w("visual studio", 13), "visual ");
        assert_eq!(w("visual studio  ", 15), "visual ");
        assert_eq!(w("visual", 6), "");
        assert_eq!(w("visual studio", 9), "visual udio"); // caret inside a word: "st|udio"
        assert_eq!(w("notepad++", 9), "notepad");
        assert_eq!(w("größe änderung", 14), "größe ");
        assert_eq!(w("", 0), "");
        assert_eq!(w("abc", 0), "abc");
        assert_eq!(w("a 😀😀", 6), "a "); // emoji are surrogate pairs in UTF-16
    }

    #[test]
    fn leading_slash_is_ignored() {
        assert_eq!(top("/vsc", APPS)[0], "Visual Studio Code");
        assert_eq!(top("  // rech", APPS)[0], "Rechner");
        assert!(top("/", APPS).is_empty());
        assert_eq!(strip_command_prefix(" /update "), "update");
    }

    #[test]
    fn shorter_name_wins_ties() {
        assert_eq!(top("edit", &["Editor Pro Max", "Editor"])[0], "Editor");
    }

    #[test]
    fn command_mode_needs_a_leading_slash() {
        assert!(is_command_query("/"));
        assert!(is_command_query("  /upd"));
        assert!(!is_command_query("upd"));
        assert!(!is_command_query("a/b"));
        assert_eq!(strip_command_prefix(" / upd "), "upd");
    }

    #[test]
    fn file_mode_needs_a_leading_percent() {
        assert!(is_file_query("%rech"));
        assert!(is_file_query(" % rech 2024"));
        assert!(!is_file_query("85 + 20%"));
        assert_eq!(strip_file_prefix(" % rech 2024 "), "rech 2024");
    }

    #[test]
    fn custom_mode_needs_a_leading_question_mark() {
        assert!(is_custom_query("?"));
        assert!(is_custom_query(" ?g rust"));
        assert!(!is_custom_query("what?"));
        assert_eq!(strip_custom_prefix(" ? g rust "), "g rust ");
    }
}
