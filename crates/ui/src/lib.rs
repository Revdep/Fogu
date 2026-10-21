//! `fogu-ui`: the iced application (tabs, split container, terminal widget, settings,
//! palette, prompts).
//!
//! One window with tabs of split panes running local shells or SSH sessions: keyboard input
//! through the keymap, scrollback, mouse selection + copy, sanitized/bracketed paste, zoom,
//! clear, tab rename/colour, a tab context menu, mouse-resizable splits, a profile picker, the
//! profile manager sidebar and the SSH prompt / host-key dialogs.

mod actions;
mod app;
mod appearance;
mod background;
mod background_menu;
mod blink;
mod color_picker;
mod context_menu;
mod drag;
mod forms;
mod forwards_dialog;
mod fuzzy_list;
mod icons;
mod keys;
mod lifecycle;
mod message;
mod metrics;
mod modal;
mod mouse;
mod notifications;
mod open_link;
mod palette;
mod pane_menu;
mod panes;
mod paste;
mod profile_editor;
mod profile_picker;
mod profiles;
mod prompt;
mod recovery;
mod restart;
mod right_click;
mod runs;
mod search;
mod secret_dialog;
mod settings;
mod sftp;
mod split_container;
mod style;
mod subscription;
mod tab;
mod tab_bar;
mod terminal_view;
mod tokens;
mod view;
mod welcome;

use std::sync::Arc;

use fogu_core::Config;
use fogu_session::SessionRuntime;

// Re-exported so callers of `run()` (the `fogu` binary) don't need their own `iced`
// dependency — `iced` stays an implementation detail of this crate.
pub use iced::Result;

pub use notifications::{Level, Notification};

/// What the binary hands to [`run`] besides the config.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Toasts to show at startup (config load errors, validation issues).
    pub notices: Vec<Notification>,
    /// Exit right after the first presented frame (benchmarks).
    pub exit_after_first_frame: bool,
    /// `--config-dir`: SSH sessions keep their `known_hosts` there (default:
    /// `$XDG_CONFIG_HOME/fogu`).
    pub config_dir: Option<std::path::PathBuf>,
    /// `$XDG_STATE_HOME/fogu`: the profile sidebar's Recent list and tab recovery files.
    pub state_dir: Option<std::path::PathBuf>,
    /// `--no-recovery`: start with one default tab, neither restore nor save the session.
    pub no_recovery: bool,
}

/// The interface font, bundled (OFL, `assets/fonts/Inter-LICENSE.txt`) so the chrome looks the
/// same whatever sans-serif the system has; the terminal keeps its own monospace font.
const INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
const INTER_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
/// The Phosphor icon font, cut to the glyphs [`icons::Icon`] names.
const PHOSPHOR: &[u8] = include_bytes!("../../../assets/fonts/Phosphor.ttf");
pub(crate) const UI_FONT: iced::Font = iced::Font::with_name("Inter");
/// Titles (toasts, dialogs).
pub(crate) const UI_MEDIUM: iced::Font = iced::Font {
    weight: iced::font::Weight::Medium,
    ..UI_FONT
};

/// `WM_CLASS` / Wayland app id; matches the `.desktop` file's `StartupWMClass`.
#[cfg(target_os = "linux")]
const APP_ID: &str = "fogu";
/// Window icon: `assets/fogu.png` at 64×64 as raw RGBA (`convert -resize 64x64 -depth 8
/// rgba:…`), so no image decoder is needed.
pub(crate) const ICON_RGBA: &[u8] = include_bytes!("../../../assets/icons/fogu-64.rgba");
pub(crate) const ICON_SIZE: u32 = 64;

/// Runs the Fogu GUI: a window whose first tab runs the default profile, configured by
/// `config` (loaded by the `fogu` binary via `fogu_core::config::load`).
///
/// # Errors
///
/// Returns an error if the session runtime cannot start or iced fails to initialise the
/// windowing or graphics backend.
pub fn run(config: Arc<Config>, options: RunOptions) -> Result {
    let runtime = Arc::new(SessionRuntime::new().map_err(iced::Error::ExecutorCreationFailed)?);
    let window = iced::window::Settings {
        size: app::WINDOW_SIZE,
        icon: iced::window::icon::from_rgba(ICON_RGBA.to_vec(), ICON_SIZE, ICON_SIZE).ok(),
        #[cfg(target_os = "linux")]
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: APP_ID.to_owned(),
            ..Default::default()
        },
        // macOS: the title bar is hidden and the content runs under it, so the native window
        // buttons sit over the tab bar (`tab_bar::TRAFFIC_LIGHTS_W`).
        #[cfg(target_os = "macos")]
        platform_specific: iced::window::settings::PlatformSpecific {
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        },
        // `Message::CloseRequested` saves the session (tab recovery), then exits.
        exit_on_close_request: false,
        // The tab bar is the title bar (move, maximize and window buttons, resize edges); macOS
        // keeps a native frame, as winit cannot resize an undecorated window there.
        decorations: cfg!(target_os = "macos"),
        // See-through terminals (`App::style` clears to transparent); read once, at creation.
        transparent: config.appearance.window_opacity < 1.0,
        blur: config.appearance.window_opacity < 1.0 && config.appearance.window_blur,
        ..Default::default()
    };
    iced::application(
        move || app::App::boot(Arc::clone(&runtime), Arc::clone(&config), options.clone()),
        app::App::update,
        app::App::view,
    )
    .title(app::App::title)
    .theme(app::App::theme)
    .style(app::App::style)
    .font(INTER_REGULAR)
    .font(INTER_MEDIUM)
    .font(PHOSPHOR)
    .default_font(UI_FONT)
    .subscription(app::App::subscription)
    .window(window)
    .run()
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_icon_is_a_valid_rgba_square() {
        assert!(
            iced::window::icon::from_rgba(
                super::ICON_RGBA.to_vec(),
                super::ICON_SIZE,
                super::ICON_SIZE
            )
            .is_ok()
        );
    }
}
