//! The fuzzy list: a scorer, a ranking over rows of plain
//! display fields, keyboard selection and one row widget. Used by the profile sidebar and the
//! profile picker, later by the command palette, so nothing here knows about profiles.

use iced::widget::{column, container, mouse_area, row, space, text};
use iced::{Alignment, Border, Color, Element, Length};

use crate::message::Message;

/// Per matched character.
const MATCH: i32 = 1;
/// A match at the start of a word (after a non-alphanumeric char, or a lower→upper change).
const WORD_START: i32 = 8;
/// A match right after the previous one.
const CONSECUTIVE: i32 = 8;
/// A match after skipped characters (not before the first match).
const GAP: i32 = -3;
/// Extra weight of a match in the title over the other fields.
const TITLE: i32 = 4;

/// One row: what is drawn and what is searched.
#[derive(Clone, Debug, Default)]
pub struct FuzzyRow {
    pub title: String,
    pub subtitle: String,
    /// Small right-aligned text (e.g. "2 h ago").
    pub detail: String,
    /// Searched too, not drawn (group name, tags…).
    pub keywords: Vec<String>,
    /// The status mark on the left.
    pub mark: Mark,
    /// A glyph before the title.
    pub icon: Option<String>,
}

/// A row's status mark: a 7 px dot in a 16 px slot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Mark {
    #[default]
    None,
    /// Not live: a hollow 1.5 px ring.
    Ring(Color),
    /// Live: a filled dot with a soft glow.
    Dot(Color),
}

/// The 16 px slot with `mark` centred in it.
pub fn mark_view<'a>(mark: Mark) -> Element<'a, Message> {
    let dot = container(space())
        .width(7)
        .height(7)
        .style(move |_theme: &iced::Theme| {
            let round = Border::default().rounded(3.5);
            match mark {
                Mark::None => container::Style::default(),
                Mark::Ring(color) => container::Style {
                    border: round.width(1.5).color(color),
                    ..container::Style::default()
                },
                Mark::Dot(color) => container::Style {
                    background: Some(color.into()),
                    border: round,
                    shadow: crate::style::glow(color, 0.8, 8.0),
                    ..container::Style::default()
                },
            }
        });
    container(dot).center_x(16).into()
}

/// How well `query` (a subsequence, case-insensitive) matches `text`: `None` = no match.
/// Rewards word starts and consecutive runs, penalises gaps; the best alignment wins (dynamic programming over
/// query × text, both short).
pub fn score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Some(0);
    }
    let orig: Vec<char> = text.chars().collect();
    let low: Vec<char> = orig
        .iter()
        .map(|&c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let word_start = |j: usize| match j.checked_sub(1).and_then(|p| orig.get(p)) {
        None => true,
        Some(&prev) => {
            !prev.is_alphanumeric()
                || (prev.is_lowercase() && orig.get(j).is_some_and(|c| c.is_uppercase()))
        }
    };
    // prev[j]: best score with the previous query char matched at text position j.
    let mut prev: Vec<Option<i32>> = Vec::new();
    for (i, &qc) in q.iter().enumerate() {
        let mut cur = vec![None; low.len()];
        let mut best_before = (i == 0).then_some(0);
        for (j, &tc) in low.iter().enumerate() {
            if tc == qc {
                let run = j
                    .checked_sub(1)
                    .and_then(|p| prev.get(p).copied().flatten())
                    .map(|s| s + CONSECUTIVE);
                let gap = best_before.map(|s| if i > 0 { s + GAP } else { s });
                if let (Some(base), Some(slot)) = (run.max(gap), cur.get_mut(j)) {
                    let bonus = if word_start(j) { WORD_START } else { 0 };
                    *slot = Some(base + MATCH + bonus);
                }
            }
            if i > 0 {
                best_before = best_before.max(prev.get(j).copied().flatten());
            }
        }
        prev = cur;
    }
    prev.into_iter().flatten().max()
}

/// Score of `row` for one query term: the best field, the title weighted up.
fn row_score(term: &str, row: &FuzzyRow) -> Option<i32> {
    let title = score(term, &row.title).map(|s| s + TITLE);
    let others = std::iter::once(&row.subtitle)
        .chain(&row.keywords)
        .filter_map(|f| score(term, f));
    others.chain(title).max()
}

/// Indices of the rows matching every whitespace-separated term of `query`, best first (ties:
/// shorter title, then original order). An empty query keeps every row in order.
pub fn rank(query: &str, rows: &[FuzzyRow]) -> Vec<usize> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return (0..rows.len()).collect();
    }
    let mut hits: Vec<(i32, usize, usize)> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, row)| {
            let total = terms
                .iter()
                .map(|t| row_score(t, row))
                .sum::<Option<i32>>()?;
            Some((total, row.title.chars().count(), i))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    hits.into_iter().map(|(_, _, i)| i).collect()
}

/// `selected` moved by `delta`, kept inside `0..len`.
pub fn step(selected: usize, delta: i8, len: usize) -> usize {
    selected
        .saturating_add_signed(delta.into())
        .min(len.saturating_sub(1))
}

/// The characters of `text` a (greedy, case-insensitive) subsequence match of `query` lands on,
/// for highlighting; empty when `query` does not match.
/// shortcut: greedy, so it may mark other letters than the best-scoring alignment would.
pub fn matched(query: &str, text: &str) -> Vec<bool> {
    let mut want = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .peekable();
    let marks: Vec<bool> = text
        .chars()
        .map(|c| {
            let hit = want
                .peek()
                .is_some_and(|&w| c.to_lowercase().next() == Some(w));
            if hit {
                want.next();
            }
            hit
        })
        .collect();
    if want.peek().is_some() {
        return vec![false; marks.len()];
    }
    marks
}

/// What a row publishes on each mouse gesture (`None` = nothing).
#[derive(Clone, Debug, Default)]
pub struct RowEvents {
    pub press: Option<Message>,
    pub double: Option<Message>,
    pub middle: Option<Message>,
    pub right: Option<Message>,
    pub enter: Option<Message>,
    pub exit: Option<Message>,
}

/// Colours a row is drawn with.
#[derive(Clone, Copy, Debug)]
pub struct RowLook {
    pub fg: Color,
    pub dim: Color,
    /// Background of the selected row.
    pub selected: Color,
    /// Background of the hovered row.
    pub hover: Color,
}

/// One row: status mark, icon, title over subtitle, then `trailing` (hover actions) or the
/// detail on the right.
pub fn row_view<'a>(
    item: &FuzzyRow,
    selected: bool,
    hovered: bool,
    look: RowLook,
    events: RowEvents,
    trailing: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut title = row![].spacing(6).align_y(Alignment::Center);
    if let Some(icon) = &item.icon {
        title = title.push(text(icon.clone()).size(14).color(look.fg));
    }
    title = title.push(
        text(item.title.clone())
            .size(13)
            .color(look.fg)
            .wrapping(text::Wrapping::None),
    );
    let label = column![
        title,
        text(item.subtitle.clone())
            .size(11.5)
            .color(look.dim)
            .wrapping(text::Wrapping::None),
    ]
    .spacing(1)
    .width(Length::Fill)
    .clip(true);
    let detail = trailing.unwrap_or_else(|| {
        text(item.detail.clone())
            .size(11)
            .color(look.dim)
            .wrapping(text::Wrapping::None)
            .into()
    });
    let body = row![mark_view(item.mark), label, detail]
        .spacing(8)
        .padding(iced::Padding {
            top: 5.0,
            right: 6.0,
            bottom: 5.0,
            left: 2.0,
        })
        .align_y(Alignment::Center);
    let bg = if selected {
        Some(look.selected)
    } else {
        hovered.then_some(look.hover)
    };
    let body = container(body)
        .width(Length::Fill)
        .style(move |_theme: &iced::Theme| container::Style {
            background: bg.map(Into::into),
            border: Border::default().rounded(crate::tokens::R_BTN),
            ..container::Style::default()
        });
    let mut area = mouse_area(body);
    if let Some(m) = events.press {
        area = area.on_press(m);
    }
    if let Some(m) = events.double {
        area = area.on_double_click(m);
    }
    if let Some(m) = events.middle {
        area = area.on_middle_press(m);
    }
    if let Some(m) = events.right {
        area = area.on_right_press(m);
    }
    if let Some(m) = events.enter {
        area = area.on_enter(m);
    }
    if let Some(m) = events.exit {
        area = area.on_exit(m);
    }
    area.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(titles: &[(&str, &str)]) -> Vec<FuzzyRow> {
        titles
            .iter()
            .map(|&(title, subtitle)| FuzzyRow {
                title: title.to_owned(),
                subtitle: subtitle.to_owned(),
                ..FuzzyRow::default()
            })
            .collect()
    }

    #[test]
    fn scorer_matches_subsequences_and_prefers_word_starts_and_runs() {
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("xyz", "web"), None);
        assert_eq!(score("bw", "web"), None, "order matters");
        assert!(score("WB", "web-backup").is_some(), "case-insensitive");
        // (query, better, worse)
        let rows = [
            ("wb", "web-backup", "lowbrow"),
            ("wb", "WebBackup", "webbackup"),
            ("db", "db-prod", "dumb"),
            ("prod", "prod-db", "p-r-o-d"),
            ("pi", "pi.lan", "api-gateway"),
        ];
        for (q, better, worse) in rows {
            assert!(
                score(q, better) > score(q, worse),
                "{q}: {better} vs {worse}"
            );
        }
    }

    #[test]
    fn rank_orders_by_score_then_shorter_title_and_needs_every_term() {
        let list = rows(&[
            ("Lowbrow box", "x@lowbrow"),
            ("Web backup", "root@10.0.0.2"),
            ("web", "me@web.lan"),
            ("Pi", "pi@10.0.0.7"),
        ]);
        // (query, expected order)
        let table: [(&str, &[usize]); 6] = [
            ("", &[0, 1, 2, 3]),
            ("wb", &[1, 0, 2]),
            ("web", &[2, 1]),
            ("10.0", &[3, 1]),
            ("web root", &[1]),
            ("nothing", &[]),
        ];
        for (q, want) in table {
            assert_eq!(rank(q, &list), want, "{q:?}");
        }
    }

    #[test]
    fn keywords_are_searched() {
        let mut list = rows(&[("A", ""), ("B", "")]);
        if let Some(row) = list.get_mut(1) {
            row.keywords = vec!["Work".to_owned()];
        }
        assert_eq!(rank("work", &list), [1]);
    }

    #[test]
    fn matched_marks_the_greedy_subsequence() {
        let marks = |q, t| -> String {
            matched(q, t)
                .iter()
                .map(|&m| if m { '^' } else { '.' })
                .collect()
        };
        assert_eq!(marks("wb", "web-backup"), "^.^.......");
        assert_eq!(marks("W B", "web"), "^.^");
        assert_eq!(marks("xz", "web"), "...");
        assert_eq!(marks("", "ab"), "..");
    }

    #[test]
    fn step_stays_in_range() {
        assert_eq!(step(0, -1, 3), 0);
        assert_eq!(step(1, 1, 3), 2);
        assert_eq!(step(2, 1, 3), 2);
        assert_eq!(step(5, 1, 0), 0);
    }
}
