//! The settings page: the nav column (an icon row per [`Page`]) on the left, the chosen page's
//! form on the right; Appearance leads with a gallery of the colour schemes.

use fogu_core::{ColorScheme, Config};
use iced::widget::{Column, Row, button, column, container, row, scrollable, space, stack, text};
use iced::{Alignment, Border, Element, Font, Length};

use super::{
    CLICKS, CURSOR, FieldId, MIN_OPACITY, OSC52, Page, RIGHT_CLICK, Settings, SettingsMsg,
};
use crate::context_menu::to_iced as rgb;
use crate::forms::{self, ERROR, Field, FieldValue, WARN};
use crate::icons::{Icon, glyph};
use crate::message::Message;
use crate::metrics::FontSpec;
use crate::palette::{Colors, Look, to_iced};
use crate::style;
use crate::tokens as t;

type Form<'a> = Element<'a, Field<FieldId>>;

/// Width of the nav column, logical px.
const NAV_W: f32 = 200.0;
/// Widest the form gets, logical px.
const MAX_W: f32 = 760.0;
/// Under fields read only when a terminal starts.
const NEW_TERMINALS: &str = "Applies to terminals opened from now on.";
/// Under fields read only when the window opens.
const AT_START: &str = "Read when Fogu starts: restart it to apply.";

/// What the page needs from the app besides the settings state.
pub struct Ctx<'a> {
    pub config: &'a Config,
    pub colors: &'a Colors,
    pub look: Look,
    /// The resolved terminal font (the font preview).
    pub font: Font,
    /// Whether the window was created transparent (opacity changes across 1 need a restart).
    pub transparent: bool,
    /// Where the changes are written.
    pub path: String,
}

/// The settings tab's page.
pub fn page<'a>(s: &'a Settings, cx: &Ctx<'a>) -> Element<'a, Message> {
    let look = cx.look;
    let form: Form<'a> = match s.page {
        Page::Appearance => appearance(s, cx),
        Page::Terminal => terminal(s, cx),
        Page::Sidebar => sidebar(s, cx),
        Page::Sftp => sftp(s, cx),
        Page::Ssh => ssh(cx),
        Page::Recovery => recovery(s, cx),
    };
    let (title, summary) = s.page.label();
    let hint = if s.page == Page::Appearance {
        "Changes apply live to every tab."
    } else {
        summary
    };
    let head = forms::page_head(look, None, title, hint);
    let form = form.map(|Field(id, value)| Message::Settings(SettingsMsg::Field(id, value)));
    let body = column![head, form]
        .spacing(22)
        .max_width(MAX_W)
        .padding([26, 32]);
    let bg = look.base;
    let content = container(
        scrollable(body)
            .direction(crate::style::thin())
            .style(crate::style::scrollbar),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_theme: &iced::Theme| container::background(bg));
    let edge = look.border;
    let rule = container(space())
        .width(1)
        .height(Length::Fill)
        .style(move |_theme: &iced::Theme| container::background(edge));
    row![nav(s.page, cx), rule, content].into()
}

/// "Settings", a row per page, then where the file is.
fn nav<'a>(current: Page, cx: &Ctx<'a>) -> Element<'a, Message> {
    let look = cx.look;
    let mut rows = column![
        container(
            text("Settings")
                .size(13.5)
                .font(crate::UI_MEDIUM)
                .color(look.fg)
        )
        .padding([4, 10])
    ]
    .spacing(2);
    for page in Page::ALL {
        let (title, _) = page.label();
        let icon = match page {
            Page::Appearance => Icon::Palette,
            Page::Terminal => Icon::Local,
            Page::Sidebar => Icon::Sidebar,
            Page::Sftp => Icon::Folder,
            Page::Ssh => Icon::Key,
            Page::Recovery => Icon::History,
        };
        rows = rows.push(forms::nav_item(
            look,
            icon,
            title,
            page == current,
            false,
            Message::Settings(SettingsMsg::Page(page)),
        ));
    }
    let foot = column![
        text("Changes are saved as you make them, to")
            .size(11)
            .color(look.dim),
        text(cx.path.clone())
            .size(11)
            .font(Font::MONOSPACE)
            .color(look.fg),
    ]
    .spacing(2)
    .padding([0, 10]);
    let bg = look.bg;
    container(column![rows, space().height(Length::Fill), foot].padding([12, 8]))
        .width(NAV_W)
        .height(Length::Fill)
        .style(move |_theme: &iced::Theme| container::background(bg))
        .into()
}

/// The field's error (red) or `note` (dim) under it.
fn under<'a>(
    s: &Settings,
    id: FieldId,
    field: Form<'a>,
    note: Option<&str>,
    look: Look,
) -> Form<'a> {
    let line = match s.errors.get(&id) {
        Some(why) => Some((why.as_str(), ERROR)),
        None => note.map(|n| (n, look.dim)),
    };
    forms::note_maybe(field, line)
}

fn num<'a>(
    s: &Settings,
    look: Look,
    id: FieldId,
    label: &str,
    value: impl std::fmt::Display,
) -> Form<'a> {
    num_note(s, look, id, label, value, None)
}

fn num_note<'a>(
    s: &Settings,
    look: Look,
    id: FieldId,
    label: &str,
    value: impl std::fmt::Display,
    note: Option<&str>,
) -> Form<'a> {
    let field = forms::number(look, id, label, &s.shown(id, value), "");
    under(s, id, field, note, look)
}

fn pick<'a, T: PartialEq>(
    look: Look,
    id: FieldId,
    label: &str,
    options: &[(T, &str)],
    value: &T,
) -> Form<'a> {
    let names = options.iter().map(|(_, n)| (*n).to_owned()).collect();
    let selected = options.iter().position(|(t, _)| t == value).unwrap_or(0);
    forms::choice(look, id, label, names, selected)
}

fn appearance<'a>(s: &'a Settings, cx: &Ctx<'a>) -> Form<'a> {
    let (look, a) = (cx.look, &cx.config.appearance);
    let fonts = s.font_choices(&a.font_family);
    let selected = fonts.iter().position(|f| *f == a.font_family).unwrap_or(0);
    let mut font_rows = vec![
        forms::choice(look, FieldId::FontFamily, "Family", fonts, selected),
        num(s, look, FieldId::FontSize, "Size (pt)", a.font_size),
    ];
    for note in &s.font_notes {
        font_rows.push(forms::note_maybe(
            space().height(0).into(),
            Some((note.as_str(), WARN)),
        ));
    }
    font_rows.push(font_preview(cx));

    let schemes = ColorScheme::builtins();
    let cards: Vec<Form<'a>> = schemes
        .iter()
        .enumerate()
        .map(|(i, scheme)| scheme_card(i, scheme, scheme.name == a.color_scheme, look))
        .collect();
    let grid: Form<'a> = Row::with_children(cards)
        .spacing(14)
        .wrap()
        .vertical_spacing(14)
        .into();

    let transparent_now = a.window_opacity < 1.0;
    let opacity_note = (transparent_now != cx.transparent)
        .then_some("Restart Fogu to make the window see-through (or opaque again).");
    let opacity = forms::slider(
        look,
        FieldId::WindowOpacity,
        "Window opacity",
        a.window_opacity,
        MIN_OPACITY..=1.0,
        percent(a.window_opacity),
    );
    let auto = a.terminal_bg_alpha.is_none();
    let mut glass = vec![
        forms::note_maybe(opacity, opacity_note.map(|n| (n, WARN))),
        forms::note(
            forms::toggle(
                look,
                FieldId::WindowBlur,
                "Blur behind the window",
                a.window_blur,
            ),
            "KDE and macOS only. Read when Fogu starts.",
            look.dim,
        ),
        forms::note(
            forms::toggle(
                look,
                FieldId::TerminalBgAuto,
                "Terminal over a tab background: automatic",
                auto,
            ),
            "Automatic: 75% over an image, see-through over a colour.",
            look.dim,
        ),
    ];
    if let Some(alpha) = a.terminal_bg_alpha {
        glass.push(forms::slider(
            look,
            FieldId::TerminalBgAlpha,
            "Terminal over a tab background",
            alpha,
            0.0..=1.0,
            percent(alpha),
        ));
    }

    column![
        forms::section(look, "Colour scheme", vec![grid]),
        forms::section(look, "Font", font_rows),
        forms::section(
            look,
            "Cursor",
            vec![
                pick(
                    look,
                    FieldId::CursorShape,
                    "Shape",
                    &CURSOR,
                    &a.cursor_shape
                ),
                forms::toggle(look, FieldId::CursorBlink, "Blink", a.cursor_blink),
            ]
        ),
        window(look, a.rounded_corners),
        forms::section(look, "Transparency", glass),
    ]
    .spacing(12)
    .into()
}

/// The "Window" section: the tiles' rounded corners.
fn window<'a>(look: Look, rounded: bool) -> Form<'a> {
    forms::section(
        look,
        "Window",
        vec![forms::note(
            forms::toggle(look, FieldId::RoundedCorners, "Rounded corners", rounded),
            "Panes, side panels and pages as rounded tiles.",
            look.dim,
        )],
    )
}

fn percent(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

/// A line of text in the terminal font, size and scheme.
fn font_preview<'a>(cx: &Ctx<'a>) -> Form<'a> {
    let a = &cx.config.appearance;
    let size = FontSpec {
        family: a.font_family.clone(),
        size_pt: a.font_size,
    }
    .size_px();
    let (fg, bg) = (
        to_iced(cx.colors.foreground()),
        to_iced(cx.colors.background()),
    );
    let sample = text("~/fogu $ ls -la  →  0O 1lI {}[] => != ==")
        .font(cx.font)
        .size(size)
        .color(fg);
    let border = cx.look.border;
    let box_ = container(sample)
        .padding([8, 10])
        .width(Length::Fill)
        .style(move |_theme: &iced::Theme| container::Style {
            background: Some(bg.into()),
            border: Border {
                color: border,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..container::Style::default()
        });
    forms::labeled(cx.look, "Preview", box_.into())
}

/// A clickable miniature terminal in `scheme`: four sample lines on its background; the chosen
/// one has an accent ring, a glow and a check.
fn scheme_card<'a>(i: usize, scheme: &ColorScheme, selected: bool, look: Look) -> Form<'a> {
    let ansi = |n: usize| rgb(scheme.ansi.get(n).copied().unwrap_or(scheme.fg));
    let (fg, bg) = (rgb(scheme.fg), rgb(scheme.bg));
    let mono = |s: &str, color| {
        text(s.to_owned())
            .font(Font::MONOSPACE)
            .size(11.5)
            .color(color)
    };
    let lines = column![
        row![
            mono("demo@prod", ansi(2)),
            mono(":", fg),
            mono("~/app", ansi(4)),
            mono(" $ ls", fg)
        ],
        row![
            mono("src  ", ansi(12)),
            mono("build.sh  ", ansi(10)),
            mono("README.md", fg)
        ],
        row![
            mono("error", ansi(1)),
            mono(": ", fg),
            mono("3 warnings", ansi(3))
        ],
        row![mono("$ ", fg), mono("▍", rgb(scheme.cursor))],
    ]
    .spacing(3);
    let accent = look.accent;
    let edge = look.border;
    let screen = container(lines)
        .padding([10, 12])
        .width(Length::Fill)
        .style(move |_theme: &iced::Theme| container::Style {
            background: Some(bg.into()),
            border: if selected {
                Border::default().rounded(t::R_PANE).width(2).color(accent)
            } else {
                Border::default().rounded(t::R_PANE).width(1).color(edge)
            },
            shadow: if selected {
                style::glow(accent, 0.3, 32.0)
            } else {
                iced::Shadow::default()
            },
            ..container::Style::default()
        });
    let screen: Element<'a, Field<FieldId>> = if selected {
        stack![
            screen,
            container(glyph(Icon::CheckCircle, 17.0, look.accent_hi))
                .padding(8)
                .width(Length::Fill)
                .align_x(Alignment::End)
        ]
        .into()
    } else {
        screen.into()
    };
    let mut name = row![text(scheme.name.clone()).size(12.5).color(look.fg)]
        .spacing(8)
        .align_y(Alignment::Center);
    if scheme.name == "Nocturne" {
        name = name.push(
            container(text("New").size(10).color(t::A100))
                .padding([1, 6])
                .style(|_theme: &iced::Theme| style::tag(t::A800)),
        );
    }
    button(column![screen, name].spacing(8))
        .padding(0)
        .width(220)
        .style(move |_theme: &iced::Theme, status| button::Style {
            text_color: if matches!(status, button::Status::Hovered) {
                look.fg
            } else {
                look.dim
            },
            ..button::Style::default()
        })
        .on_press(Field(FieldId::Scheme, FieldValue::Choice(i)))
        .into()
}

#[expect(clippy::too_many_lines, reason = "one row per setting")]
fn terminal<'a>(s: &'a Settings, cx: &Ctx<'a>) -> Form<'a> {
    let (look, t) = (cx.look, &cx.config.terminal);
    let toggle = |id, label, value| forms::toggle(look, id, label, value);
    let words = forms::text_field(
        look,
        FieldId::WordSeparators,
        "Word separators",
        &t.word_separators,
        "",
    );
    column![
        forms::section(
            look,
            "Scrollback and selection",
            vec![
                num_note(
                    s,
                    look,
                    FieldId::Scrollback,
                    "Scrollback lines",
                    t.scrollback_lines,
                    Some(NEW_TERMINALS)
                ),
                toggle(
                    FieldId::CopyOnSelect,
                    "Copy the selection on select",
                    t.copy_on_select
                ),
                under(
                    s,
                    FieldId::WordSeparators,
                    words,
                    Some("Ends a double-click selection. New terminals."),
                    look
                ),
            ]
        ),
        forms::section(
            look,
            "Paste",
            vec![
                toggle(
                    FieldId::PasteOnMiddleClick,
                    "Middle click pastes the selection",
                    t.paste_on_middle_click
                ),
                toggle(
                    FieldId::WarnMultilinePaste,
                    "Confirm a multi-line paste",
                    t.warn_on_multiline_paste
                ),
                toggle(
                    FieldId::TrimPaste,
                    "Trim surrounding whitespace",
                    t.trim_whitespace_on_paste
                ),
                toggle(
                    FieldId::ReplaceNewlines,
                    "Newlines become spaces",
                    t.replace_newlines_on_paste
                ),
                toggle(
                    FieldId::BracketedPaste,
                    "Bracketed paste when asked",
                    t.bracketed_paste
                ),
            ]
        ),
        forms::section(
            look,
            "Mouse and keyboard",
            vec![
                pick(
                    look,
                    FieldId::RightClick,
                    "Right click",
                    &RIGHT_CLICK,
                    &t.right_click
                ),
                num(
                    s,
                    look,
                    FieldId::WheelLines,
                    "Lines per wheel notch",
                    t.wheel_lines
                ),
                toggle(FieldId::AltIsMeta, "Alt sends Esc (meta)", t.alt_is_meta),
                toggle(
                    FieldId::ScrollOnInput,
                    "Scroll to the bottom on input",
                    t.scroll_on_input
                ),
                num(
                    s,
                    look,
                    FieldId::PaneResizeStep,
                    "Pane resize step (share)",
                    t.pane_resize_step
                ),
            ]
        ),
        forms::section(
            look,
            "Programs",
            vec![
                pick(
                    look,
                    FieldId::Osc52,
                    "Programs writing the clipboard",
                    &OSC52,
                    &t.osc52_write
                ),
                toggle(
                    FieldId::KeepHistory,
                    "Keep the screen on restart / reconnect",
                    t.keep_history_on_restart
                ),
            ]
        ),
    ]
    .spacing(12)
    .into()
}

fn sidebar<'a>(s: &'a Settings, cx: &Ctx<'a>) -> Form<'a> {
    let (look, a) = (cx.look, &cx.config.appearance);
    forms::section(
        look,
        "Profile sidebar",
        vec![
            forms::toggle(
                look,
                FieldId::ShowBuiltins,
                "Show built-in shells",
                a.show_builtin_profiles,
            ),
            num_note(
                s,
                look,
                FieldId::RecentProfiles,
                "Recent profiles",
                a.recent_profiles,
                Some("0 hides the Recent section."),
            ),
            num(
                s,
                look,
                FieldId::SidebarWidth,
                "Width (px)",
                a.profiles_sidebar_width,
            ),
            pick(
                look,
                FieldId::ProfilesClick,
                "Opening a profile",
                &CLICKS,
                &a.profiles_click,
            ),
        ],
    )
}

#[expect(clippy::too_many_lines, reason = "one row per setting")]
fn sftp<'a>(s: &'a Settings, cx: &Ctx<'a>) -> Form<'a> {
    let (look, f) = (cx.look, &cx.config.sftp);
    let toggle = |id, label, value| forms::toggle(look, id, label, value);
    let dir = f
        .download_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let max = f
        .max_download_bytes
        .map(|n| n.to_string())
        .unwrap_or_default();
    let max = forms::number(
        look,
        FieldId::MaxDownload,
        "Largest download (bytes)",
        &s.shown(FieldId::MaxDownload, max),
        "no limit",
    );
    let mut panel = vec![
        toggle(FieldId::ShowHidden, "Show dot files", f.show_hidden),
        num(
            s,
            look,
            FieldId::PanelWidth,
            "Width (share of the tab)",
            f.panel_width,
        ),
        toggle(
            FieldId::FollowCwd,
            "Follow the terminal's directory",
            f.follow_cwd,
        ),
        num(
            s,
            look,
            FieldId::TransferLinger,
            "Finished transfers stay (s)",
            f.transfer_linger_secs,
        ),
    ];
    // Only where Wayland and XWayland exist; macOS and Windows take drops natively.
    if cfg!(all(unix, not(target_os = "macos"))) {
        panel.push(forms::note(
            toggle(
                FieldId::WaylandDragDrop,
                "Enable drag & drop on Wayland",
                f.wayland_drag_and_drop,
            ),
            "Fogu will open on XWayland. Restart required.",
            WARN,
        ));
    }
    column![
        text("SFTP panels opened from now on use these values.")
            .size(13)
            .color(look.dim),
        forms::section(look, "Panel", panel),
        forms::section(
            look,
            "Downloads",
            vec![
                forms::path(
                    look,
                    FieldId::DownloadDir,
                    "Download folder",
                    &dir,
                    "~/Downloads",
                    s.no_picker.map_or(Ok(()), Err)
                ),
                toggle(
                    FieldId::KeepPartial,
                    "Keep .part files of failed downloads",
                    f.keep_partial
                ),
                toggle(
                    FieldId::KeepExecutable,
                    "Keep the executable bit",
                    f.keep_executable_bit
                ),
                toggle(
                    FieldId::AllowGrowing,
                    "Allow files growing while downloaded",
                    f.allow_growing_files
                ),
                under(s, FieldId::MaxDownload, max, None, look),
                num(
                    s,
                    look,
                    FieldId::FreeSpace,
                    "Free space to leave (bytes)",
                    f.free_space_reserve_bytes
                ),
            ]
        ),
        forms::section(
            look,
            "Limits",
            vec![
                num(
                    s,
                    look,
                    FieldId::MaxParallel,
                    "Files at once",
                    f.max_parallel_files
                ),
                num(
                    s,
                    look,
                    FieldId::RequestTimeout,
                    "Request timeout (s)",
                    f.request_timeout_secs
                ),
                num(
                    s,
                    look,
                    FieldId::MaxListing,
                    "Entries per folder",
                    f.max_listing_entries
                ),
                num(
                    s,
                    look,
                    FieldId::MaxWalkEntries,
                    "Entries per recursive transfer",
                    f.max_walk_entries
                ),
                num(
                    s,
                    look,
                    FieldId::MaxWalkDepth,
                    "Depth of a recursive transfer",
                    f.max_walk_depth
                ),
            ]
        ),
    ]
    .spacing(12)
    .into()
}

fn ssh<'a>(cx: &Ctx<'a>) -> Form<'a> {
    let look = cx.look;
    forms::section(
        look,
        "SSH",
        vec![forms::note(
            forms::toggle(
                look,
                FieldId::ReadSshConfig,
                "Show ~/.ssh/config hosts",
                cx.config.ssh.read_ssh_config,
            ),
            "Every plain Host becomes a read-only profile (\"SSH config\" in the sidebar).",
            look.dim,
        )],
    )
}

fn recovery<'a>(s: &'a Settings, cx: &Ctx<'a>) -> Form<'a> {
    let (look, r) = (cx.look, &cx.config.recovery);
    let rows: Vec<Form<'a>> = vec![
        forms::note(
            forms::toggle(
                look,
                FieldId::RecoveryEnabled,
                "Restore the tabs on restart",
                r.enabled,
            ),
            AT_START,
            look.dim,
        ),
        forms::toggle(
            look,
            FieldId::RecoveryScrollback,
            "Also restore the scrollback (local shells)",
            r.restore_scrollback,
        ),
        forms::toggle(
            look,
            FieldId::RecoverySshScrollback,
            "Also restore SSH scrollback, encrypted",
            r.restore_ssh_scrollback,
        ),
        num(
            s,
            look,
            FieldId::RecoveryLines,
            "Scrollback lines saved per pane",
            r.scrollback_lines,
        ),
    ];
    let warning = text("Local scrollback may hold passwords shown in the terminal; it is stored unencrypted, readable only by you.")
        .size(12)
        .color(WARN);
    let ssh_note = text("SSH scrollback is locked with a key kept in your system keyring; without it, SSH tabs come back empty.")
        .size(12)
        .color(look.dim);
    Column::new()
        .push(forms::section(look, "Tab recovery", rows))
        .push(warning)
        .push(ssh_note)
        .spacing(8)
        .into()
}
