//! The settings page: the nav column (an icon row per [`Page`]) on the left, the chosen page's
//! form on the right; Appearance leads with a gallery of the colour schemes.

use fogu_core::{ColorScheme, Config};
use iced::widget::{
    Row, button, column, container, row, scrollable, space, stack, text, text_editor,
};
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
/// Height of the config.toml editor, logical px (the page scrolls; the editor scrolls inside).
const FILE_EDITOR_H: f32 = 520.0;
/// Under fields read only when a terminal starts.
const NEW_TERMINALS: &str = "Applies to terminals opened from now on.";
/// Under fields read only when the window opens.
const AT_START: &str = "Read when Fogu starts: restart it to apply.";
/// Above the bundled licences: where the third-party crate notices are.
const LICENSES_HINT: &str =
    "Run `fogu --licenses` for the licences of every crate built into Fogu.";

/// What the page needs from the app besides the settings state.
pub struct Ctx<'a> {
    pub updater: &'a crate::updater::Updater,
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
    let field = |form: Form<'a>| {
        form.map(|Field(id, value)| Message::Settings(SettingsMsg::Field(id, value)))
    };
    let form: Element<'a, Message> = match s.page {
        Page::Appearance => field(appearance(s, cx)),
        Page::Terminal => field(terminal(s, cx)),
        Page::Sidebar => field(sidebar(s, cx)),
        Page::Sftp => field(sftp(s, cx)),
        Page::Ssh => field(ssh(cx)),
        Page::Recovery => field(recovery(s, cx)),
        Page::File => file_page(s, look),
        Page::Updates => crate::updater::view(cx.updater, look),
        // The crate notices are ~500 KB: one text widget that size stalls the UI thread.
        Page::Licenses => licenses(look),
    };
    let (title, summary) = s.page.label();
    let hint = if s.page == Page::Appearance {
        "Changes apply live to every tab."
    } else {
        summary
    };
    let head = forms::page_head(look, None, title, hint);
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
            Page::Recovery | Page::Licenses => Icon::History,
            Page::Updates => Icon::Download,
            Page::File => Icon::FileCode,
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
    if scheme.name == ColorScheme::default().name {
        name = name.push(
            container(text("Default").size(10).color(t::A100))
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
            "Fogu will open on XWayland: on HiDPI screens it may look small or blurry, KDE's \
             window blur is off and input methods (IBus, fcitx5) need XMODIFIERS. Restart required.",
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
        forms::note(
            forms::toggle(
                look,
                FieldId::RecoverySshScrollback,
                "Also restore SSH scrollback, encrypted",
                r.restore_ssh_scrollback,
            ),
            "Locked with a key kept in your system keyring; without it, SSH tabs come back empty.",
            look.dim,
        ),
        num(
            s,
            look,
            FieldId::RecoveryLines,
            "Scrollback lines saved per pane",
            r.scrollback_lines,
        ),
    ];
    forms::section(look, "Tab recovery", rows)
}

/// `config.toml` as text: Save writes it only when it loads (the error otherwise), Revert reads
/// the file again.
fn file_page(s: &Settings, look: Look) -> Element<'_, Message> {
    let edit = |action| Message::Settings(SettingsMsg::FileEdit(action));
    let changed = s.file.text() != s.file_saved;
    let editor = text_editor(&s.file)
        .on_action(edit)
        .font(Font::MONOSPACE)
        .size(13)
        .height(FILE_EDITOR_H)
        .padding(10);
    let buttons = row![
        button(text("Save").size(13))
            .style(style::btn(look, style::Btn::Primary))
            .on_press_maybe(changed.then_some(Message::Settings(SettingsMsg::FileSave))),
        button(text("Revert").size(13))
            .style(style::btn(look, style::Btn::Secondary))
            .on_press_maybe(changed.then_some(Message::Settings(SettingsMsg::FileRevert))),
    ]
    .spacing(8);
    let mut page = column![
        text("Changes apply when you save, like an edit in another editor. A file that would not load is not saved.")
            .size(12)
            .color(look.dim),
        editor,
        buttons,
    ]
    .spacing(12);
    if let Some(why) = &s.file_error {
        page = page.push(text(why.as_str()).size(12).color(ERROR));
    }
    page.into()
}

/// `THIRD_PARTY_LICENSES` (Markdown) as a page: headings, paragraphs and bullets reflowed to the
/// page width, `**`/backtick markup dropped. Only what that one file uses.
fn licenses<'a>(look: Look) -> Element<'a, Message> {
    let mut page = column![text(LICENSES_HINT).size(12).color(look.dim)].spacing(10);
    for block in markdown_blocks(include_str!("../../../../THIRD_PARTY_LICENSES")) {
        page = page.push(match block {
            Block::Heading(1, line) => text(line).size(18).font(crate::UI_MEDIUM).color(look.fg),
            Block::Heading(_, line) => text(line).size(14.5).font(crate::UI_MEDIUM).color(look.fg),
            Block::Bullet(line) => text(format!("•  {line}"))
                .size(12.5)
                .color(look.fg)
                // `--unicodes=…` is one long word: break it rather than run off the page.
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            Block::Text(line) => text(line)
                .size(12.5)
                .color(look.fg)
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
        });
    }
    page.into()
}

/// A Markdown block of [`licenses`].
#[derive(Debug, PartialEq)]
enum Block {
    Heading(usize, String),
    Bullet(String),
    Text(String),
}

/// Splits `md` into blocks, joining hard-wrapped lines so the text wraps at the page width.
fn markdown_blocks(md: &str) -> Vec<Block> {
    let plain = |s: &str| s.replace("**", "").replace('`', "");
    let mut blocks: Vec<Block> = Vec::new();
    let mut open = false;
    for line in md.lines() {
        let trimmed = line.trim();
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if trimmed.is_empty() {
            open = false;
        } else if hashes > 0 && trimmed[hashes..].starts_with(' ') {
            blocks.push(Block::Heading(hashes, plain(trimmed[hashes..].trim())));
            open = false;
        } else if let Some(item) = trimmed.strip_prefix("- ") {
            blocks.push(Block::Bullet(plain(item)));
            open = true;
        } else if let (true, Some(Block::Bullet(last) | Block::Text(last))) =
            (open, blocks.last_mut())
        {
            last.push(' ');
            last.push_str(&plain(trimmed));
        } else {
            blocks.push(Block::Text(plain(trimmed)));
            open = true;
        }
    }
    blocks
}

#[cfg(test)]
mod licenses_tests {
    use super::{Block, markdown_blocks};

    #[test]
    fn markdown_blocks_reflow_wrapped_lines_and_drop_markup() {
        let md = "# Title\n\nIntro `code`\ncontinues.\n\n## Fonts\n\n- **Inter** (`a.ttf`,\n  b.ttf)\n- Next\n";
        assert_eq!(
            markdown_blocks(md),
            [
                Block::Heading(1, "Title".into()),
                Block::Text("Intro code continues.".into()),
                Block::Heading(2, "Fonts".into()),
                Block::Bullet("Inter (a.ttf, b.ttf)".into()),
                Block::Bullet("Next".into()),
            ]
        );
    }
}
