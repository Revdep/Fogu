//! The panel's directory listing: entries as the engine sent them plus the view state (sort,
//! hidden files, filter, selection, keyboard cursor, scroll). Pure: no iced, no I/O.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use fogu_session::display_name;
use fogu_session::sftp::{Entry, EntryKind};

/// Sortable column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// File name (case-insensitive).
    Name,
    /// Size in bytes.
    Size,
    /// Modification time.
    Modified,
    /// Permission bits.
    Mode,
}

/// Modifier keys held during a click.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClickMods {
    /// Ctrl: toggle one entry.
    pub ctrl: bool,
    /// Shift: select the range from the anchor.
    pub shift: bool,
}

/// One directory as shown in the panel.
#[derive(Clone, Debug)]
pub struct Listing {
    /// The entries, raw (names are attacker data).
    pub entries: Vec<Entry>,
    /// The server had more entries than `max_listing_entries`.
    pub truncated: bool,
    pub sort: SortKey,
    pub descending: bool,
    pub show_hidden: bool,
    /// Case-insensitive substring filter.
    pub filter: String,
    /// Selected entry names.
    pub selected: BTreeSet<String>,
    /// Shift-click anchor.
    anchor: Option<String>,
    /// Keyboard cursor (the last clicked/moved-to entry).
    pub cursor: Option<String>,
}

impl Listing {
    /// An empty listing.
    pub fn new(show_hidden: bool) -> Self {
        Self {
            entries: Vec::new(),
            truncated: false,
            sort: SortKey::Name,
            descending: false,
            show_hidden,
            filter: String::new(),
            selected: BTreeSet::new(),
            anchor: None,
            cursor: None,
        }
    }

    /// Replaces the entries (a new directory or a refresh); keeps the selection of names that
    /// still exist.
    pub fn replace(&mut self, entries: Vec<Entry>, truncated: bool) {
        self.entries = entries;
        self.truncated = truncated;
        let names: BTreeSet<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        self.selected.retain(|n| names.contains(n.as_str()));
        if self
            .cursor
            .as_ref()
            .is_some_and(|c| !names.contains(c.as_str()))
        {
            self.cursor = None;
        }
        if self
            .anchor
            .as_ref()
            .is_some_and(|c| !names.contains(c.as_str()))
        {
            self.anchor = None;
        }
    }

    /// Clears the selection, cursor and filter (entering another directory).
    pub fn reset_view(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
        self.filter.clear();
    }

    /// Click on a column header: same column flips the order, another column sorts ascending.
    pub fn sort_by(&mut self, key: SortKey) {
        if self.sort == key {
            self.descending = !self.descending;
        } else {
            self.sort = key;
            self.descending = false;
        }
    }

    /// Dot files currently left out because `show_hidden` is off.
    pub fn hidden_count(&self) -> usize {
        if self.show_hidden {
            return 0;
        }
        self.entries
            .iter()
            .filter(|e| e.name.starts_with('.'))
            .count()
    }

    /// The entries to show, in order: hidden/filter applied, directories first, then by the
    /// sort column (ties by name).
    pub fn visible(&self) -> Vec<&Entry> {
        let needle = self.filter.to_lowercase();
        let mut out: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| self.show_hidden || !e.name.starts_with('.'))
            .filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle))
            .collect();
        out.sort_by(|a, b| {
            let dirs = is_dir(b).cmp(&is_dir(a));
            dirs.then_with(|| {
                let by = self.compare(a, b);
                if self.descending { by.reverse() } else { by }
            })
        });
        out
    }

    fn compare(&self, a: &Entry, b: &Entry) -> Ordering {
        let by_name = || {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        };
        match self.sort {
            SortKey::Name => by_name(),
            SortKey::Size => a.size.cmp(&b.size).then_with(by_name),
            SortKey::Modified => a.mtime.cmp(&b.mtime).then_with(by_name),
            SortKey::Mode => a.mode.cmp(&b.mode).then_with(by_name),
        }
    }

    /// Click on entry `name` with `mods` (plain = select only it).
    pub fn click(&mut self, name: &str, mods: ClickMods) {
        if mods.shift
            && let Some(anchor) = self.anchor.clone()
        {
            let names: Vec<String> = self.visible().iter().map(|e| e.name.clone()).collect();
            let a = names.iter().position(|n| *n == anchor);
            let b = names.iter().position(|n| n == name);
            if let (Some(a), Some(b)) = (a, b) {
                if !mods.ctrl {
                    self.selected.clear();
                }
                let range = names.get(a.min(b)..=a.max(b)).unwrap_or_default();
                self.selected.extend(range.iter().cloned());
                self.cursor = Some(name.to_owned());
                return;
            }
        }
        if mods.ctrl {
            if !self.selected.remove(name) {
                self.selected.insert(name.to_owned());
            }
        } else {
            self.selected.clear();
            self.selected.insert(name.to_owned());
        }
        self.anchor = Some(name.to_owned());
        self.cursor = Some(name.to_owned());
    }

    /// Arrow keys: moves the cursor `delta` rows (clamped) and selects only that entry.
    /// Returns the new cursor's row.
    pub fn move_cursor(&mut self, delta: i32) -> Option<usize> {
        let names: Vec<String> = self.visible().iter().map(|e| e.name.clone()).collect();
        if names.is_empty() {
            return None;
        }
        let current = self
            .cursor
            .as_ref()
            .and_then(|c| names.iter().position(|n| n == c));
        let last = names.len() - 1;
        let row = match current {
            None if delta < 0 => last,
            None => 0,
            Some(i) => i.saturating_add_signed(delta as isize).min(last),
        };
        let name = names.get(row)?.clone();
        self.click(&name, ClickMods::default());
        Some(row)
    }

    /// Right-press on `name`: an entry outside the selection becomes the only selected one;
    /// inside it, the selection stays (the menu acts on all of it).
    pub fn right_click(&mut self, name: &str) {
        if !self.selected.contains(name) {
            self.click(name, ClickMods::default());
        }
        self.cursor = Some(name.to_owned());
    }

    /// The keyboard cursor's entry, if it is shown.
    pub fn cursor_entry(&self) -> Option<&Entry> {
        let cursor = self.cursor.as_deref()?;
        self.visible().into_iter().find(|e| e.name == cursor)
    }

    /// The selected entries, in display order.
    pub fn selected_entries(&self) -> Vec<&Entry> {
        self.visible()
            .into_iter()
            .filter(|e| self.selected.contains(&e.name))
            .collect()
    }
}

/// Directories and symlinks to directories (navigable).
pub fn is_dir(e: &Entry) -> bool {
    e.kind == EntryKind::Dir
        || (e.kind == EntryKind::Symlink && e.link_kind == Some(EntryKind::Dir))
}

/// A name as shown in the list: sanitized, `/` after directories.
pub fn shown_name(e: &Entry) -> String {
    let mut name = display_name(&e.name);
    if is_dir(e) {
        name.push('/');
    }
    name
}

/// `1.5 KiB`-style size; directories show nothing.
pub fn human_size(e: &Entry) -> String {
    match (is_dir(e), e.size) {
        (true, _) | (_, None) => String::new(),
        (false, Some(bytes)) => human_bytes(bytes),
    }
}

/// Bytes in binary units with one decimal (`512 B`, `1.5 KiB`, `3.0 GiB`).
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    #[expect(clippy::cast_precision_loss, reason = "shown with one decimal")]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let name = UNITS.get(unit).copied().unwrap_or("B");
    if unit == 0 {
        format!("{bytes} {name}")
    } else {
        format!("{value:.1} {name}")
    }
}

/// `drwxr-xr-x` from the kind and mode bits (`?` when the server sent no mode).
pub fn permissions(e: &Entry) -> String {
    let Some(mode) = e.mode else {
        return "?".to_owned();
    };
    let kind = match e.kind {
        EntryKind::Dir => 'd',
        EntryKind::Symlink => 'l',
        EntryKind::File => '-',
        EntryKind::Other => '?',
    };
    let mut out = String::with_capacity(10);
    out.push(kind);
    for shift in [6u32, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        out.push(if bits & 4 != 0 { 'r' } else { '-' });
        out.push(if bits & 2 != 0 { 'w' } else { '-' });
        let special = match shift {
            6 => mode & 0o4000 != 0,
            3 => mode & 0o2000 != 0,
            _ => mode & 0o1000 != 0,
        };
        let exec = bits & 1 != 0;
        out.push(match (special, exec, shift) {
            (true, true, 0) => 't',
            (true, false, 0) => 'T',
            (true, true, _) => 's',
            (true, false, _) => 'S',
            (false, true, _) => 'x',
            (false, false, _) => '-',
        });
    }
    out
}

/// `YYYY-MM-DD HH:MM` (UTC) from seconds since the epoch; empty when unknown.
pub fn modified(e: &Entry) -> String {
    e.mtime
        .map(|secs| utc_minutes(i64::from(secs)))
        .unwrap_or_default()
}

/// `YYYY-MM-DD HH:MM` (UTC) from seconds since the epoch.
pub fn utc_minutes(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian (H. Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
#[path = "listing_tests.rs"]
mod tests;
