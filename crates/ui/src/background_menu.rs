//! Tab background UI: the tab menu's "Background ▸" sub-menu, its messages, "Save to profile",
//! keeping the decode cache in step with the tabs, and what the view draws.

use std::path::PathBuf;
use std::sync::Arc;

use fogu_core::{
    Background, Fit, ProfileId, RgbColor, TabBackground, config, effective_background,
};
use iced::widget::operation;
use iced::{Point, Size, Task};

use crate::app::App;
use crate::background::{Backdrop, Decoded, Key, LoadError, decode, px};
use crate::context_menu::{
    ContextMenu, MENU_INPUT, MenuAction, MenuItem, MenuSlider, PRESET_COLORS, to_iced,
};
use crate::message::Message;
use crate::notifications::{Level, Notification};
use crate::palette::Colors;
use crate::sftp::{no_picker_note, picker_available};
use crate::tab::Tab;

/// Opacity slider range, in percent.
const OPACITY_PCT: std::ops::RangeInclusive<f32> = 20.0..=100.0;
/// 1/N of a preset colour is mixed into the scheme background for the solid-colour presets
/// (full-strength tab colours would make text unreadable).
const PRESET_TINT_DIV: i32 = 5;

/// Background menu items and decode results.
#[derive(Clone, Debug)]
pub enum BgMsg {
    /// "Background ▸": swap the tab menu for the background menu.
    Menu,
    /// "Solid color…": show the colour palette.
    Colors,
    /// A solid colour was picked.
    SetColor(RgbColor),
    /// "Image…": open the file picker.
    PickImage,
    /// The picker closed (`None` = cancelled).
    ImagePicked(Option<PathBuf>),
    /// The opacity slider moved (percent).
    Opacity(f32),
    /// A fit was chosen.
    Fit(Fit),
    /// "None": no background on this tab.
    Clear,
    /// "Save to profile".
    SaveToProfile,
    /// A decode finished.
    Loaded(Key, Result<Decoded, LoadError>),
}

impl From<BgMsg> for Message {
    fn from(msg: BgMsg) -> Self {
        Self::Background(msg)
    }
}

impl App {
    pub(crate) fn on_background(&mut self, msg: BgMsg) -> Task<Message> {
        let anchor = self.menu.as_ref().map_or(self.tab_bar_cursor, |m| m.anchor);
        match msg {
            BgMsg::Menu => self.menu = Some(self.background_menu(anchor)),
            BgMsg::Colors => {
                let presets = tinted_presets(&self.colors);
                let presets: Vec<_> = presets.iter().map(|(n, c)| (*n, *c)).collect();
                let pick = |c| BgMsg::SetColor(c).into();
                self.menu = Some(ContextMenu::palette(anchor, &presets, None, pick));
                return operation::focus(MENU_INPUT);
            }
            BgMsg::SetColor(color) => {
                self.menu = None;
                self.set_tab_background(TabBackground::Set(Background::Color(color)));
            }
            BgMsg::PickImage => {
                self.menu = None;
                return pick_image(self);
            }
            BgMsg::ImagePicked(Some(path)) => {
                self.backgrounds.forget(&path);
                let (opacity, fit) = self.current_image().unwrap_or_else(|| {
                    let new = Background::image(PathBuf::new());
                    (new.opacity(), Fit::default())
                });
                self.set_tab_background(TabBackground::Set(Background::Image {
                    path,
                    opacity,
                    fit,
                    blur: false,
                }));
            }
            BgMsg::ImagePicked(None) => {}
            BgMsg::Opacity(pct) => self.edit_image(anchor, |opacity, _| *opacity = pct / 100.0),
            BgMsg::Fit(new) => self.edit_image(anchor, |_, fit| *fit = new),
            BgMsg::Clear => {
                self.menu = None;
                self.set_tab_background(TabBackground::None);
            }
            BgMsg::SaveToProfile => {
                self.menu = None;
                return self.save_background();
            }
            BgMsg::Loaded(key, result) => {
                let error = result.as_ref().err().cloned();
                if !self.backgrounds.finish(&key, result)
                    && let Some(error) = error
                {
                    let name = key.path.file_name().unwrap_or(key.path.as_os_str());
                    let detail = format!("{}: {error}", name.to_string_lossy());
                    return self.notify(
                        Notification::new(Level::Error, "Background image not shown")
                            .detail(detail),
                    );
                }
            }
        }
        Task::none()
    }

    fn set_tab_background(&mut self, background: TabBackground) {
        let active = self.active;
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == active) {
            tab.background = background;
        }
    }

    /// The active tab's image opacity and fit, if it shows an image.
    fn current_image(&self) -> Option<(f32, Fit)> {
        let tab = self.active_tab()?;
        let effective =
            effective_background(&tab.background, &tab.profile, &self.config.appearance)?;
        match effective.background {
            Background::Image { fit, .. } => Some((effective.background.opacity(), *fit)),
            Background::Color(_) => None,
        }
    }

    /// Changes the active tab's image opacity/fit (a tab override from then on) and refreshes
    /// the open menu.
    fn edit_image(&mut self, anchor: Point, edit: impl FnOnce(&mut f32, &mut Fit)) {
        let active = self.active;
        let appearance = &self.config.appearance;
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == active) else {
            return;
        };
        let current = effective_background(&tab.background, &tab.profile, appearance)
            .map(|e| (e.background.clone(), e.background.opacity()));
        if let Some((
            Background::Image {
                path, fit, blur, ..
            },
            opacity,
        )) = current
        {
            let (mut opacity, mut fit) = (opacity, fit);
            edit(&mut opacity, &mut fit);
            tab.background = TabBackground::Set(Background::Image {
                path,
                opacity,
                fit,
                blur,
            });
        }
        if self.menu.is_some() {
            self.menu = Some(self.background_menu(anchor));
        }
    }

    /// The background sub-menu for the active tab.
    fn background_menu(&self, anchor: Point) -> ContextMenu {
        let tab = self.active_tab();
        let image = self.current_image();
        let has_background = tab.is_some_and(|t| {
            effective_background(&t.background, &t.profile, &self.config.appearance).is_some()
        });
        let ui = |msg: BgMsg| MenuAction::Ui(msg.into());
        let mut items = vec![
            MenuItem::new("Solid color…", ui(BgMsg::Colors)),
            MenuItem::new("Image…", ui(BgMsg::PickImage)),
        ];
        for (i, (label, fit)) in [
            ("Cover", Fit::Cover),
            ("Contain", Fit::Contain),
            ("Tile", Fit::Tile),
        ]
        .into_iter()
        .enumerate()
        {
            let mark = if image.is_some_and(|(_, f)| f == fit) {
                "  ✓"
            } else {
                ""
            };
            let mut item = MenuItem::new(format!("Fit: {label}{mark}"), ui(BgMsg::Fit(fit)));
            item.enabled = image.is_some();
            item.separator = i == 0;
            items.push(item);
        }
        let mut none = MenuItem::new("None", ui(BgMsg::Clear));
        none.separator = true;
        none.enabled = has_background;
        items.push(none);
        let mut save = MenuItem::new("Save to profile", ui(BgMsg::SaveToProfile));
        save.enabled = tab.is_some_and(|t| self.is_stored(&t.profile.id));
        if !save.enabled {
            save.tooltip = Some("Only for tabs opened from a saved profile");
        }
        items.push(save);
        let mut menu = ContextMenu::new(anchor, items);
        menu.slider = image.map(|(opacity, _)| {
            let pct = (opacity * 100.0).round();
            MenuSlider {
                label: format!("Image opacity: {pct:.0} %"),
                range: OPACITY_PCT,
                value: pct.clamp(*OPACITY_PCT.start(), *OPACITY_PCT.end()),
                on_change: |pct| BgMsg::Opacity(pct).into(),
            }
        });
        menu
    }

    fn is_stored(&self, id: &ProfileId) -> bool {
        self.config.profiles.iter().any(|p| &p.id == id)
    }

    /// Writes the active tab's background into its profile in the config file, then into the
    /// running config and every open tab of that profile.
    fn save_background(&mut self) -> Task<Message> {
        let Some(tab) = self.active_tab() else {
            return Task::none();
        };
        let id = tab.profile.id.clone();
        let name = tab.profile.name.clone();
        let value = match &tab.background {
            TabBackground::Profile => tab.profile.background.clone(),
            TabBackground::None => None,
            TabBackground::Set(background) => Some(background.clone()),
        };
        let dir = config::config_dir(self.config_dir.as_deref());
        // Edits only the `background` key in place; refuses on a file with an error or without
        // this profile (never rewrites the user's file from the typed config).
        if let Err(why) = config::set_profile_background(&dir, &id.0, value.as_ref()) {
            let note =
                Notification::new(Level::Error, "Background not saved").detail(why.to_string());
            return self.notify(note);
        }
        self.remember_config_file();
        let config = Arc::make_mut(&mut self.config);
        if let Some(profile) = config.profiles.iter_mut().find(|p| p.id == id) {
            profile.background.clone_from(&value);
        }
        let active = self.active;
        for tab in self.tabs.iter_mut().filter(|t| t.profile.id == id) {
            tab.profile.background.clone_from(&value);
            if tab.id == active {
                tab.background = TabBackground::Profile;
            }
        }
        self.notify(Notification::new(Level::Info, "Background saved").detail(name))
    }

    /// Starts decoding every image a tab shows that is not cached (or was decoded for a smaller
    /// window); drops decodes no tab uses.
    pub(crate) fn sync_backgrounds(&mut self) -> Task<Message> {
        let appearance = &self.config.appearance;
        let wanted: Vec<Key> = self
            .tabs
            .iter()
            .filter_map(|t| {
                match effective_background(&t.background, &t.profile, appearance)?.background {
                    Background::Image { path, fit, .. } => Some(Key::new(path, *fit)),
                    Background::Color(_) => None,
                }
            })
            .collect();
        let px = |logical: f32| px(f64::from(logical * self.scale));
        let target = Size::new(px(self.window_size.width), px(self.window_size.height));
        let missing = self.backgrounds.missing(&wanted, target);
        Task::batch(missing.into_iter().map(|key| {
            let (path, fit_to) = (key.path.clone(), (!key.full_size).then_some(target));
            let job = self
                .runtime
                .handle()
                .spawn_blocking(move || decode(&path, fit_to));
            Task::perform(job, move |joined| {
                let result = joined
                    .unwrap_or_else(|_| Err(LoadError::Corrupt("the decoder crashed".to_owned())));
                BgMsg::Loaded(key.clone(), result).into()
            })
        }))
    }

    /// Alpha of the terminal area: `appearance.window_opacity` once the window was created
    /// transparent, else opaque.
    pub(crate) fn window_alpha(&self) -> f32 {
        if !self.window_transparent {
            return 1.0;
        }
        let opacity = self.config.appearance.window_opacity;
        opacity.clamp(crate::settings::MIN_OPACITY, 1.0)
    }

    /// What `tab` draws under its panes, and the alpha of the terminal's default background.
    /// An image still loading (or that failed) draws nothing: the terminal stays opaque, unless
    /// the window is see-through (the split's fill then carries the opacity).
    pub(crate) fn backdrop(&self, tab: &Tab) -> (Option<Backdrop>, f32) {
        let bare = if self.window_alpha() < 1.0 { 0.0 } else { 1.0 };
        let Some(effective) =
            effective_background(&tab.background, &tab.profile, &self.config.appearance)
        else {
            return (None, bare);
        };
        let backdrop = match effective.background {
            Background::Color(color) => Backdrop::Color(iced::Color {
                a: self.window_alpha(),
                ..to_iced(*color)
            }),
            Background::Image { path, fit, .. } => {
                let Some(image) = self.backgrounds.get(&Key::new(path, *fit)) else {
                    return (None, bare);
                };
                Backdrop::Image {
                    handle: image.handle.clone(),
                    size: image.size,
                    opacity: effective.background.opacity(),
                    fit: *fit,
                }
            }
        };
        (Some(backdrop), effective.terminal_bg_alpha)
    }
}

fn pick_image(app: &mut App) -> Task<Message> {
    if !picker_available() {
        return app.notify(no_picker_note());
    }
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("Background image")
        .add_filter("Images (PNG, JPEG, WebP)", &["png", "jpg", "jpeg", "webp"]);
    Task::perform(dialog.pick_file(), |file| {
        BgMsg::ImagePicked(file.map(|f| f.path().to_path_buf())).into()
    })
}

/// The tab colour presets mixed 1/[`PRESET_TINT_DIV`] into the scheme background: dark tints on a
/// dark scheme, light ones on a light scheme, so the scheme's text stays readable.
fn tinted_presets(colors: &Colors) -> Vec<(&'static str, RgbColor)> {
    let bg = colors.background();
    let mix = |base: u8, tint: u8| {
        let v = i32::from(base) + (i32::from(tint) - i32::from(base)) / PRESET_TINT_DIV;
        u8::try_from(v).unwrap_or(base)
    };
    PRESET_COLORS
        .iter()
        .map(|&(name, hex)| {
            let c = RgbColor::from_hex(hex);
            let tinted = RgbColor {
                r: mix(bg.r, c.r),
                g: mix(bg.g, c.g),
                b: mix(bg.b, c.b),
            };
            (name, tinted)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]

    use super::*;

    #[test]
    fn save_to_profile_keeps_comments_and_refuses_broken_files() {
        let dir = std::env::temp_dir().join(format!("fogu-bg-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        let text = "# mine\n[[profiles]]\nid = \"work\"\nname = \"Work\"\ntype = \"local\"\n";
        std::fs::write(&file, text).unwrap();
        let bg = Background::image(PathBuf::from("/w.png"));
        config::set_profile_background(&dir, "work", Some(&bg)).unwrap();
        let saved = std::fs::read_to_string(&file).unwrap();
        assert!(saved.starts_with(text), "{saved}");
        let (config, err) = config::load(&dir);
        assert!(err.is_none());
        assert_eq!(config.profiles[0].background, Some(bg.clone()));
        assert!(config::set_profile_background(&dir, "gone", None).is_err());
        std::fs::write(&file, "not toml [[[").unwrap();
        assert!(config::set_profile_background(&dir, "work", Some(&bg)).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "not toml [[[");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn presets_are_tinted_towards_the_scheme_background() {
        let colors = Colors::new(&fogu_core::ColorScheme::default());
        let bg = colors.background();
        for (_, c) in tinted_presets(&colors) {
            // Within 20 % + rounding of the background on every channel.
            for (a, b) in [(c.r, bg.r), (c.g, bg.g), (c.b, bg.b)] {
                assert!(a.abs_diff(b) <= 52, "{c:?}");
            }
        }
    }
}
