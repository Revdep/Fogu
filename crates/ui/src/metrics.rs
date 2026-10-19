//! Terminal font resolution and cell metrics.
//!
//! The family is resolved explicitly through cosmic-text's fontdb (it ignores fontconfig
//! aliases), falling back to the first monospace face; `cell_w = advance('0')`,
//! `cell_h = ascent + descent + line_gap`, both rounded to whole device pixels; glyphs are drawn
//! at the size whose advance is exactly `cell_w`, so text stays on the grid.

use fogu_term::{Column, Line, Point, Side};
use iced::advanced::graphics::text::cosmic_text::fontdb::{self, Database, Query};
use iced::advanced::graphics::text::cosmic_text::skrifa::instance::{LocationRef, Size};
use iced::advanced::graphics::text::cosmic_text::skrifa::{FontRef, MetadataProvider as _};
use iced::advanced::graphics::text::font_system;
use iced::{Font, Vector};

/// Requested terminal font (`appearance.font_family` / `font_size`, zoom applied).
#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    /// Family name; `monospace` resolves to fontdb's monospace family.
    pub family: String,
    /// Size in points (1 pt = 4/3 logical px).
    pub size_pt: f32,
}

impl Default for FontSpec {
    fn default() -> Self {
        Self {
            family: "monospace".to_owned(),
            size_pt: 12.0,
        }
    }
}

impl FontSpec {
    /// Size in logical pixels.
    pub fn size_px(&self) -> f32 {
        self.size_pt * 4.0 / 3.0
    }
}

/// Metrics of the resolved face in em (multiply by the font size in px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontUnits {
    /// Advance of `'0'` in em.
    pub advance: f32,
    /// `ascent + descent + line_gap` in em.
    pub line_height: f32,
}

impl FontUnits {
    /// Typical monospace proportions, used when no face could be resolved.
    pub const FALLBACK: Self = Self {
        advance: 0.6,
        line_height: 1.2,
    };
}

/// A resolved terminal font: what iced draws with plus its unit metrics.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedFont {
    /// Font handed to `fill_text`.
    pub font: Font,
    /// Unit metrics of the regular face.
    pub units: FontUnits,
}

/// Cell size for one font size and scale factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    /// Text size in logical px: the requested size nudged (by under half a device px of
    /// advance) so a glyph's advance is exactly `cell_w`. Otherwise a run of N cells drawn at
    /// the font's own advance drifts off the grid (a few cells on a long line).
    pub font_size: f32,
    /// Cell width in logical px (a whole number of device px).
    pub cell_w: f32,
    /// Cell height in logical px (a whole number of device px).
    pub cell_h: f32,
    /// Cell width in device px.
    pub cell_w_px: u16,
    /// Cell height in device px.
    pub cell_h_px: u16,
}

impl CellMetrics {
    /// Scales `units` to `font_size` logical px at `scale` device px per logical px.
    pub fn new(units: FontUnits, font_size: f32, scale: f32) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let (width, width_px) = round_to_device(units.advance * font_size, scale);
        let (height, height_px) = round_to_device(units.line_height * font_size, scale);
        let fitted = width / units.advance;
        Self {
            font_size: if fitted.is_finite() && fitted > 0.0 {
                fitted
            } else {
                font_size
            },
            cell_w: width,
            cell_h: height,
            cell_w_px: width_px,
            cell_h_px: height_px,
        }
    }
}

/// Number of whole cells that fit in `bounds`; never below 2×1 (a 0×0 or 1-column PTY confuses
/// shells).
pub fn grid_size(bounds: iced::Size, metrics: &CellMetrics) -> (u16, u16) {
    let fit = |len: f32, cell: f32| {
        let n = (len / cell).floor().clamp(1.0, f32::from(u16::MAX));
        #[expect(clippy::cast_sign_loss, reason = "clamped to 1..=u16::MAX above")]
        let n = n as u16;
        n
    };
    (
        fit(bounds.width, metrics.cell_w).max(2),
        fit(bounds.height, metrics.cell_h),
    )
}

/// Rounds a logical length to whole device pixels (at least 1). Returns the snapped logical
/// length and the device-pixel count.
pub fn round_to_device(logical: f32, scale: f32) -> (f32, u16) {
    let px = (logical * scale).round().clamp(1.0, f32::from(u16::MAX));
    // `px` is in 1..=u16::MAX after the clamp.
    #[expect(clippy::cast_sign_loss, reason = "clamped to 1..=u16::MAX above")]
    let px_u = px as u16;
    (px / scale, px_u)
}

/// A viewport cell under the mouse (row 0 = top visible row) and which half of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellPos {
    /// Column, 0-based.
    pub col: usize,
    /// Viewport row, 0-based.
    pub row: usize,
    /// Left or right half of the cell (selection anchoring).
    pub side: Side,
}

/// Maps `offset` (px from the grid's top-left corner) to the cell under it, clamped to the
/// `cols` × `rows` grid: left of the grid is column 0's left half, right of it the last column's
/// right half, above/below it the first/last row.
pub fn pixel_to_cell(offset: Vector, m: &CellMetrics, cols: usize, rows: usize) -> CellPos {
    let index = |len: f32, cell: f32, count: usize| {
        let last = count.saturating_sub(1);
        if len.is_nan() || len < 0.0 {
            return (0, false);
        }
        let n = (len / cell).floor();
        #[expect(
            clippy::cast_sign_loss,
            reason = "non-negative; `as` saturates huge values"
        )]
        let n = n as usize;
        if n > last { (last, true) } else { (n, false) }
    };
    let (col, past_right) = index(offset.x, m.cell_w, cols);
    let (row, _) = index(offset.y, m.cell_h, rows);
    let in_cell = offset.x - cells_f32(col) * m.cell_w;
    let side = if past_right || in_cell > m.cell_w / 2.0 {
        Side::Right
    } else {
        Side::Left
    };
    CellPos { col, row, side }
}

/// Grid point of a viewport cell while the view is scrolled `display_offset` lines back
/// (grid line 0 is the top of the live screen, history lines are negative).
pub fn viewport_to_point(pos: CellPos, display_offset: usize) -> Point {
    let row = i32::try_from(pos.row).unwrap_or(i32::MAX);
    let offset = i32::try_from(display_offset).unwrap_or(i32::MAX);
    Point::new(Line(row.saturating_sub(offset)), Column(pos.col))
}

/// A cell count as `f32` (grids are at most `u16::MAX` cells wide/high).
pub fn cells_f32(n: usize) -> f32 {
    f32::from(u16::try_from(n).unwrap_or(u16::MAX))
}

/// A face found by [`find_face`].
struct Found {
    id: fontdb::ID,
    name: String,
    /// `false` when the requested family was missing and the first monospace face stands in.
    exact: bool,
}

/// Resolves `family` (and the `fallbacks`, for diagnostics) in iced's global font database and
/// reads the primary face's metrics. Returns the font plus human-readable problems (missing
/// families) for the UI to show as toasts. The chosen faces and their files are logged at info
/// level so the user can see what `monospace` resolved to.
pub fn resolve(family: &str, fallbacks: &[String]) -> (ResolvedFont, Vec<String>) {
    let mut system = font_system()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cfg!(target_os = "macos") {
        // cosmic-text's generic monospace is Noto Sans Mono, which stock macOS lacks.
        system.raw().db_mut().set_monospace_family("Menlo");
    }
    let db = system.raw().db();
    let mut problems = Vec::new();
    for fallback in fallbacks {
        match find_face(db, fallback) {
            Some(found) if found.exact => log_face("fallback font", fallback, db, &found),
            _ => {
                tracing::warn!(family = fallback, "fallback font not found");
                problems.push(format!("Fallback font \"{fallback}\" not found"));
            }
        }
    }
    let Some(found) = find_face(db, family) else {
        tracing::warn!(
            family,
            "no monospace font found; using iced's default monospace"
        );
        problems.push(format!("No monospace font found for \"{family}\""));
        let resolved = ResolvedFont {
            font: Font::MONOSPACE,
            units: FontUnits::FALLBACK,
        };
        return (resolved, problems);
    };
    // The generic family resolving to "some monospace face" is normal, not a user error.
    if !found.exact && !family.eq_ignore_ascii_case("monospace") {
        problems.push(format!(
            "Font \"{family}\" not found; using \"{}\"",
            found.name
        ));
    }
    log_face("terminal font", family, db, &found);
    let units = db
        .with_face_data(found.id, read_units)
        .flatten()
        .unwrap_or_else(|| {
            tracing::warn!(
                font = found.name,
                "could not read font metrics; using defaults"
            );
            FontUnits::FALLBACK
        });
    // iced fonts name their family with `&'static str`; one small leak per resolved family.
    let name: &'static str = Box::leak(found.name.into_boxed_str());
    let resolved = ResolvedFont {
        font: Font::with_name(name),
        units,
    };
    (resolved, problems)
}

fn log_face(what: &str, requested: &str, db: &Database, found: &Found) {
    let source = db.face(found.id).map(|face| match &face.source {
        fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => {
            path.display().to_string()
        }
        fontdb::Source::Binary(_) => "<embedded>".to_owned(),
    });
    tracing::info!(requested, font = found.name, ?source, "{what}");
}

/// Explicit lookup of `family` (or fontdb's monospace family for `monospace`), else the first
/// monospace face in the database.
fn find_face(db: &Database, family: &str) -> Option<Found> {
    let wanted = if family.eq_ignore_ascii_case("monospace") {
        fontdb::Family::Monospace
    } else {
        fontdb::Family::Name(family)
    };
    let query = db.query(&Query {
        families: &[wanted],
        ..Query::default()
    });
    let exact = query.is_some();
    let id = query.or_else(|| {
        // Prefer a regular face: the first monospace entry in database order may be a Bold or
        // Italic file, whose family would then be drawn regular anyway.
        let candidates = || {
            db.faces()
                .filter(|face| face.monospaced && !face.post_script_name.contains("Emoji"))
        };
        let regular = candidates().find(|face| {
            face.weight == fontdb::Weight::NORMAL && face.style == fontdb::Style::Normal
        });
        let first = regular.or_else(|| candidates().next())?;
        tracing::warn!(
            requested = family,
            "font family not found; using first monospace face"
        );
        Some(first.id)
    })?;
    let face = db.face(id)?;
    let name = face.families.first().map(|(name, _)| name.clone())?;
    Some(Found { id, name, exact })
}

fn read_units(data: &[u8], index: u32) -> Option<FontUnits> {
    let font = FontRef::from_index(data, index).ok()?;
    let size = Size::unscaled();
    let metrics = font.metrics(size, LocationRef::default());
    let upem = f32::from(metrics.units_per_em);
    let glyph = font.charmap().map('0')?;
    let advance = font
        .glyph_metrics(size, LocationRef::default())
        .advance_width(glyph)?;
    if upem <= 0.0 || advance <= 0.0 {
        return None;
    }
    // skrifa reports descent as a negative offset below the baseline.
    let line_height = (metrics.ascent - metrics.descent + metrics.leading) / upem;
    Some(FontUnits {
        advance: advance / upem,
        line_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_to_whole_device_pixels() {
        let cases = [
            // (logical, scale) → (logical, px)
            ((9.6, 1.0), (10.0, 10)),
            ((9.4, 1.0), (9.0, 9)),
            ((9.6, 2.0), (9.5, 19)),
            ((9.6, 1.25), (9.6, 12)),
            ((9.6, 1.5), (9.333_333, 14)),
            ((0.1, 1.0), (1.0, 1)),
        ];
        for ((logical, scale), (want_logical, want_px)) in cases {
            let (got_logical, got_px) = round_to_device(logical, scale);
            assert_eq!(got_px, want_px, "{logical} @ {scale}");
            assert!(
                (got_logical - want_logical).abs() < 1e-4,
                "{logical} @ {scale}"
            );
            // The snapped length is always a whole number of device pixels.
            let device = got_logical * scale;
            assert!((device - device.round()).abs() < 1e-4);
        }
    }

    #[test]
    fn cell_metrics_scale_units() {
        let units = FontUnits {
            advance: 0.6,
            line_height: 1.17,
        };
        let m = CellMetrics::new(units, 16.0, 1.0);
        assert_eq!((m.cell_w_px, m.cell_h_px), (10, 19));
        assert!((m.cell_w - 10.0).abs() < f32::EPSILON);
        // Glyphs are drawn at the size whose advance is exactly one cell (10 px, not 9.6).
        assert!((m.font_size * units.advance - m.cell_w).abs() < 1e-4);
        let hidpi = CellMetrics::new(units, 16.0, 2.0);
        assert_eq!((hidpi.cell_w_px, hidpi.cell_h_px), (19, 37));
        assert!((hidpi.font_size * units.advance - hidpi.cell_w).abs() < 1e-4);
        // A broken scale factor falls back to 1.0.
        assert_eq!(CellMetrics::new(units, 16.0, 0.0), m);
    }

    #[test]
    fn pixel_to_cell_table() {
        let m = CellMetrics::new(FontUnits::FALLBACK, 16.0, 1.0);
        // cell 10 × 19, grid 80 × 24
        let (l, r) = (Side::Left, Side::Right);
        let cases = [
            ((0.0, 0.0), (0, 0, l)),
            ((4.9, 18.9), (0, 0, l)),
            ((5.1, 19.0), (0, 1, r)),
            ((10.0, 0.0), (1, 0, l)),
            ((799.0, 455.0), (79, 23, r)),
            ((5000.0, 5000.0), (79, 23, r)),
            ((-30.0, -30.0), (0, 0, l)),
            ((f32::NAN, 40.0), (0, 2, l)),
        ];
        for ((x, y), (col, row, side)) in cases {
            let got = pixel_to_cell(Vector::new(x, y), &m, 80, 24);
            assert_eq!(got, CellPos { col, row, side }, "({x}, {y})");
        }
    }

    #[test]
    fn viewport_rows_map_to_grid_lines() {
        let pos = |row| CellPos {
            col: 3,
            row,
            side: Side::Left,
        };
        let cases = [(0, 0, 0), (5, 0, 5), (0, 50, -50), (23, 50, -27)];
        for (row, offset, line) in cases {
            let p = viewport_to_point(pos(row), offset);
            assert_eq!(
                (p.line, p.column),
                (Line(line), Column(3)),
                "{row} @ {offset}"
            );
        }
    }

    #[test]
    fn grid_size_floors_and_clamps() {
        let m = CellMetrics::new(FontUnits::FALLBACK, 16.0, 1.0);
        // cell 10 × 19
        assert_eq!(grid_size(iced::Size::new(805.0, 460.0), &m), (80, 24));
        assert_eq!(grid_size(iced::Size::new(3.0, 0.0), &m), (2, 1));
    }

    #[test]
    fn twelve_points_is_sixteen_pixels() {
        assert!((FontSpec::default().size_px() - 16.0).abs() < f32::EPSILON);
    }
}
