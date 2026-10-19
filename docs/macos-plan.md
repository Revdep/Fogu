# macOS support plan (Intel first, Apple Silicon in CI)

Status: proposal, nothing implemented. Written against branch `work` at commit `7658bb7`,
reviewed 2026-10-19. Claims cite `file:line` of this
repository or of a crate in the local cargo registry (`~/.cargo/registry/src/index.crates.io-*/`,
versions as pinned in `Cargo.lock`). Items marked **unverified** could not be checked here: only
`x86_64-unknown-linux-gnu` is installed (`rustup target list --installed`), so no `cargo check
--target *-apple-darwin` was run.

## Decisions taken (do not reopen)

- **Keymap:** macOS defaults follow Tabby's `configDefaults.macos.yaml` and
  `tabby-terminal/src/config.ts`, for the actions Fogu has (item 14). Tabby actions Fogu lacks
  are listed there as deferred, not implemented.
- **Distribution:** plain executable installed by a script. `.app`, signing, notarization,
  DMG and Homebrew cask are deferred (see "Deferred").
- **Test machine:** the owner's Hackintosh (Intel, `x86_64-apple-darwin`) is the primary target
  and the only place anything is run; `aarch64-apple-darwin` is covered by CI only. GPU/Metal on
  a Hackintosh may be odd, so every manual check runs twice: normal and `fogu --software-render`
  (`crates/app/src/cli.rs:30`, re-execs with `ICED_BACKEND=tiny-skia`).

## Pace and bottleneck

Fogu (~54k lines of Rust, ~550 tests) was written in under 7 days with AI assistance against a
human-scale estimate of 300-450 days, so the earlier "5-6.5 working days" below was human
pace. Real pace: **about 5-7 hours of authoring in total**, in the phase breakdown below. The
bottleneck is not writing code: nothing here can be compiled for macOS on the Linux box, so
each phase ends in one or more **Hackintosh test cycles** (pull, `cargo build --release`, run
the manual checklist twice, report back). Expect ~8 cycles in all; wall-clock time is whatever
the owner's cycle time is.

## Summary

The code is closer to portable than its "Linux-only" labels suggest. The GUI stack (iced 0.14 /
winit 0.30 / wgpu) already resolves for `aarch64-apple-darwin` in `Cargo.lock` (`cargo tree
--target aarch64-apple-darwin -p fogu` pulls `metal 0.32`, `objc2-app-kit`, `clipboard_macos`;
no `x11rb`, `wayland-client` or `smithay-clipboard`), `pty-process 0.5.3` supports macOS (only
`spawn_borrowed` is `cfg(not(target_os = "macos"))`, `pty-process-0.5.3/src/command.rs:161`; we
use `spawn`, `crates/session/src/local.rs:279`), and all `std::os::unix` permission code is POSIX.

Two spots do not compile on macOS (`rustix` `prctl`; iced's per-OS `PlatformSpecific` struct).
About fifteen more compile but misbehave (`/proc` reads, `xdg-open`, the picker check, GNU
`tail --pid`, `XDG_RUNTIME_DIR`, no Secret Service, no primary selection, no edge resize on an
undecorated window, Option-as-Meta, the default monospace font). Most fixes are one `cfg!()`
branch each.

Estimated total: 5-7 hours of authoring plus ~8 Hackintosh test cycles to a usable unsigned
binary with macOS CI (see "Pace and bottleneck").

Rule for all gates: prefer `cfg!(target_os = "macos")` in an `if`/`match` (both branches compile
on both OSes, so clippy pedantic `-D warnings` never sees dead code) over `#[cfg]` on items.
Use `#[cfg]` only where a type or function does not exist on the other OS (items 1, 2, 9), and
then gate the whole function or constant, not a statement, so nothing is left unused.

## Inventory of Linux-specific spots

Severity: **B** = build break, **F** = functional break (compiles, wrong behaviour), **U** = UX /
convention, **P** = packaging / CI / docs.

| # | Sev | Location | Today | On macOS | Fix |
|---|-----|----------|-------|----------|-----|
| 1 | B | `crates/app/src/main.rs:53-63` `harden_process` | `rustix::process::set_dumpable_behavior(NotDumpable)` in release builds | `rustix` exports `prctl` only under `cfg(linux_kernel)` (`rustix-1.1.5/src/process/mod.rs:35-36,97-98`): compile error | `#[cfg(target_os = "linux")]` body as today; `#[cfg(not(target_os = "linux"))]` body: `rustix::process::setrlimit(Resource::Core, Rlimit { current: Some(0), maximum: Some(0) })` (rlimit is available on Apple, `mod.rs:46-54`). `PT_DENY_ATTACH` needs `libc` FFI, which `unsafe_code = "forbid"` (`Cargo.toml:122`) rules out, and no safe crate wraps it. State in the doc comment that macOS gets core dumps off but not anti-attach. |
| 2 | B | `crates/ui/src/lib.rs:107-110` | `PlatformSpecific { application_id: APP_ID, .. }` | iced picks the struct per OS (`iced_core-0.14.0/src/window/settings.rs:6-8`); the macOS one has only `title_hidden`, `titlebar_transparent`, `fullsize_content_view` (`settings/macos.rs:5-12`): compile error | `fn platform_specific() -> PlatformSpecific` with a `#[cfg(target_os = "linux")]` body (uses `APP_ID`) and a `#[cfg(not(..))]` body returning `Default::default()`. Move `APP_ID` into the Linux body so it is not unused on macOS. |
| 3 | F | `crates/ui/src/lib.rs:114` (`decorations: false`), `crates/ui/src/tab_bar.rs:189,236-248` (custom window buttons), `:302-335` (`resize_edges`), `crates/ui/src/view.rs:215`, `crates/ui/src/app.rs:255-264` | The tab bar is the title bar; edge strips call `window::drag_resize` | winit's macOS backend returns `NotSupported` from `drag_resize_window` (`winit-0.30.13/src/platform_impl/macos/window_delegate.rs:1179-1181`), so Fogu's edge strips do nothing. The window itself stays natively resizable: winit creates undecorated windows with `Borderless \| Resizable \| Miniaturizable` (`window_delegate.rs:535-543`), so AppKit's own edge resize may still work (**unverified**). `drag_window` (`:1170`) and `set_maximized` (`:1256`) work. | Phase 1: `decorations: cfg!(target_os = "macos")` (today `false`; Linux keeps the custom tab-bar title); skip `resize_edges()` and the minimize/maximize/close buttons when `cfg!(target_os = "macos")`. The native title bar sits above the tab bar: plain, zero layout work. Fallback to try in the same test cycle if the double title bar looks bad: keep `decorations: false` on macOS too and only skip `resize_edges()` (relies on AppKit edge resize). Phase 2 polish (optional): `title_hidden` + `titlebar_transparent` + `fullsize_content_view` from item 2 with a ~75 px left inset for the traffic lights; rendering under a transparent title bar in iced 0.14 is **unverified**. |
| 4 | F | `crates/session/src/local.rs:44-52` `current_cwd`, `:54-64` `foreground` | `/proc/<pid>/cwd` (OSC 7 fallback for "new tab here" / SFTP follow) and `/proc/<pid>/comm` (`WatchForeground`, `crates/session/src/lib.rs:172-176`) | No `/proc`: both always `None`. Shells report OSC 7 only when configured to, so "new tab here" loses the cwd for a stock zsh. | Phase 1: `if cfg!(target_os = "macos") { return None; }` (no dead code). No test: today's `.ok()?` (`:49,61`) already yields `None` on macOS, so a re-added bare `/proc` read is indistinguishable until phase 4's positive tests (cwd/program of the test process itself). Phase 4: `libproc` crate (`proc_pidinfo(PROC_PIDVNODEPATHINFO)` for cwd, `proc_name` for the program; MIT, unsafe inside the crate only) under `[target.'cfg(target_os = "macos")'.dependencies]` in `crates/session/Cargo.toml`. Spawning `lsof`/`ps` instead is slower (`lsof` takes seconds on macOS) and not worth it. |
| 5 | U | `crates/core/src/profile.rs:290` (`login: ... .unwrap_or(false)`) | "Default shell" starts a non-login shell | A Finder-launched app gets launchd's minimal env (`PATH=/usr/bin:/bin:/usr/sbin:/sbin`, no `LANG`); zsh reads `.zprofile` (Homebrew `PATH`, locale) only as a login shell. Terminal.app and iTerm2 default to login shells. | `pub const DEFAULT_LOGIN: bool = cfg!(target_os = "macos");` in `profile.rs`, used at `:290` and by the editor checkbox at `crates/ui/src/profile_editor/view.rs:321` (which duplicates the `unwrap_or(false)` today and would otherwise show the wrong state). The `/etc/shells` profiles already use `login: Some(true)` (`crates/core/src/builtin.rs:66-67`). Harmless when Fogu is started from Terminal (nested login shell, same as Terminal.app). |
| 6 | F | `crates/session/src/pipeline.rs:262-266` `local_hostname` | Reads `/proc/sys/kernel/hostname` for `is_local_host` | Always empty: one fewer "local" alias (documented as safe, `:260-261`) | `rustix::system::uname().nodename()`; add `"system"` to the `rustix` features (`Cargo.toml:52`). Works on both OSes, drops the `/proc` read. |
| 7 | F | `crates/session/src/ssh/agent.rs:158-204` `start_private` | Starts `ssh-agent -a <sock> tail --pid=<us> -f /dev/null` under `$XDG_RUNTIME_DIR/fogu` | (a) BSD `tail` has no `--pid`: the child exits at once and the agent with it. (b) `XDG_RUNTIME_DIR` is unset (`:161-164`): returns `None` before trying. | (a) Replace the `tail` argv (`:184-189`) with `sh -c 'while kill -0 "$1" 2>/dev/null; do sleep 5; done' _ <pid>`: same structure (agent exits with its child), works on both OSes, no process to manage. (Holding a `-D` child with `kill_on_drop` in a `OnceLock` would not work: statics are never dropped.) (b) Base dir: `$XDG_RUNTIME_DIR`, else `$TMPDIR` (macOS gives a per-user `0700` dir under `/var/folders/`); keep the owner-and-`0700` check (`:170-177`). |
| 8 | F | `crates/session/src/secrets.rs:15,25,48,60-74` (`oo7` D-Bus Secret Service), `crates/session/Cargo.toml:25,31` | Stored passwords/passphrases go to the freedesktop Secret Service | `oo7` has no target cfg and compiles (zbus is pure Rust), but no service exists: every profile gets `Disabled("no Secret Service answers (is KWallet or GNOME Keyring running?)")`. Misleading text; no stored secrets on macOS. | Phase 1: at the top of `detect()`, `if cfg!(target_os = "macos") { return Self::Disabled("stored passwords are not supported on macOS yet".into()); }`. One line, `oo7` stays compiled everywhere (dead weight in the mac binary, acceptable). Phase 4: `SecretStore::Keychain` via `security-framework` (generic passwords, service `fogu`, account = the full `SecretKey::attributes()` map (`crates/core/src/secret_key.rs:54-72`) serialised with `serde_json` (already a workspace dependency, `Cargo.toml:66`; `BTreeMap` gives a deterministic key order and JSON escaping makes it injective, unlike `k=v;` joining, since `name` and `profile_id` are unrestricted strings, `:67-69`), which is exactly what the Secret Service lookup matches on (`secrets.rs:194`); unit test with a value containing `;`, `=` and `"`; `label()` (`:78-93`) truncates fingerprints to 12 chars and is for display only; safe API, MIT/Apache; **unverified**: not in the local registry) under a macOS target table, and only then move `oo7` to `[target.'cfg(target_os = "linux")'.dependencies]` and `#[cfg]` the `SecretService` variant. Verified: Cargo accepts `ssh = ["dep:oo7"]` for an optional dependency in a target table (tested with a scratch crate on cargo 1.99). Keep the no-fallback policy (`:6-9`). |
| 9 | F | `crates/ui/src/sftp/desktop.rs:110-125,196-213` (`xdg_open`), `crates/ui/src/open_link.rs:18`, `crates/ui/src/notifications.rs:211-219` ("Show in file manager" opens the directory) | Opens files, directories and URLs with `xdg-open` | `xdg-open` does not exist: every Open / Show in file manager / link click fails with "No such file" | `const OPENER: &str = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };` used for the program and in the two error strings. `open <dir>` already shows the directory in Finder (`open -R <file>` to select the file is optional polish). `open` also launches by type, so `LAUNCHABLE` (`:141`) gains `app`, `pkg`, `dmg`, `command`, `terminal`, `workflow`, `scpt`. `refuse_open` (`:176-194`) checks the extension only inside `meta.is_file() && ..` (`:186`), and `.app`/`.workflow` bundles are directories: hoist it to `if launchable \|\| (meta.is_file() && mode & 0o111 != 0)`. Test: a `foo.app` directory is refused. |
| 10 | F | `crates/ui/src/sftp/desktop.rs:217-249` `picker_available`/`portal_installed`, `crates/ui/src/profile_editor/handlers.rs:41` `NO_PICKER` | Opens an `rfd` dialog only if `zenity` or the xdg portal service file exists | Both false on macOS, so Upload / Download to / Browse show "Install xdg-desktop-portal or zenity", although `rfd` builds its `NSOpenPanel` backend on macOS regardless of features (`rfd-0.17.2/src/backend.rs:31-32`) and runs it on the main thread via `dispatch2::run_on_main` (`backend/macos/modal_future.rs:87`), which works under winit's run loop (**unverified** at runtime) | `picker_available()`: `cfg!(target_os = "macos") || in_path("zenity") || portal_installed()`. Nothing else changes; the `xdg-portal` feature (`Cargo.toml:107`) is inert on macOS. |
| 11 | U | `crates/ui/src/sftp/desktop.rs:95-105` `wayland_drop_hint`, `crates/ui/src/view.rs:279-282` | Drops described as "X11 only" | winit's macOS backend emits `DroppedFile` (`window_delegate.rs:413`), so drops work; the hint keys off `WAYLAND_DISPLAY` and never fires | Comments/README only. |
| 12 | F | `crates/ui/src/lifecycle.rs:51-56`, `crates/ui/src/app.rs:437-438,508-511`, `crates/ui/src/mouse.rs:160-164`, `crates/ui/src/pane_menu.rs:31,64` | Copy-on-select writes the primary selection; middle click and "Paste selection" read it | `window_clipboard`'s non-Linux providers keep the trait defaults, which return `None` (`window_clipboard-0.5.1/src/lib.rs:85-96`): copy-on-select is a silent no-op, middle-click paste yields nothing | Open question A. Recommended (iTerm2 behaviour): three call sites use `iced::clipboard::{write_primary,read_primary}` directly (`lifecycle.rs:54` OSC 52 selection, `app.rs:510` copy-on-select, `app.rs:438` middle-click/"Paste selection"), so add two one-line wrappers in `crates/ui/src/` (`write_selection`/`read_selection`: primary on Linux, the clipboard on macOS) and route all three through them. Leave the "Paste selection" menu entry (it then pastes the clipboard). |
| 13 | F | `crates/ui/src/keys.rs:103-129` `key_input` (uses `modified_key`), caller `crates/ui/src/terminal_view.rs:515-523`, `crates/term/src/input/keys.rs:174-191` `encode_char`, `crates/core/src/config/mod.rs:152,181` `alt_is_meta = true` | Alt + letter sends `ESC` + letter | iced hands `key` = `key_without_modifiers()` and `modified_key` = the logical key (`iced_winit-0.14.1/src/conversion.rs:235-240,275`). On macOS Option+d has logical key and text `∂`, so Fogu sends `ESC ∂`: readline `M-f`/`M-b`, Emacs and tmux are broken by default. Hotkeys are fine (`hotkey()` tries `key` first, `keys.rs:21`). winit's `OptionAsAlt` would fix this at the source but iced 0.14 does not expose it (no `option_as_alt` in `iced_winit`/`iced_core`). | `key_input` (`keys.rs:105`) takes neither `key` nor `alt_is_meta`; the setting is only applied later in `encode_key` (`app.rs:420`). Pass both in (`terminal_view.rs:515-523` has `key`; the caller reads `config.terminal.alt_is_meta`). When `cfg!(target_os = "macos") && alt_is_meta && modifiers.alt()` and `key` is a single ASCII letter, build `KeyCode::Char` from it (`to_ascii_uppercase` when `modifiers.shift()`) and drop `text`, so `encode_char` prefixes `ESC` to the plain letter. Anything else (punctuation, non-ASCII, `alt_is_meta = false`) keeps today's `modified_key` path; note that with `alt_is_meta = true` `encode_char` (`crates/term/src/input/keys.rs:184-189`) still prefixes `ESC` to that single character, so Option+`;` (which the US layout turns into `…`) sends `ESC …` (`1b e2 80 a6`) and never the bare character — same as Alt on Linux today, and the reason `alt_is_meta = false` exists. Known limit: on a US layout Option+e/u/i/n/` are dead keys that winit hands to IME before any `KeyboardInput` (`winit-0.30.13/src/platform_impl/macos/view.rs:458-468`; iced enables IME for the window, `iced_winit-0.14.1/src/window.rs:283`), so `M-e`/`M-u` cannot be sent; document it. Fixing that needs winit's `OptionAsAlt`, which iced 0.14 does not expose: deferred, not worth a patched iced. Unit tests next to `keys.rs:183` on the encoded bytes: Option+f → `ESC f`, Option+Shift+f → `ESC F`, Option+`;` with `modified_key`/text `…` → `ESC …`, setting off → bare `…`. |
| 14 | U | `crates/core/src/action.rs:294-342` `default_keymap` (Ctrl-Shift-*, Alt-n, F11), `crates/core/src/keychord.rs:1-2,157-160,218-232`, `README.md:135-141` | Linux terminal shortcuts; modifier names `ctrl/alt/shift/meta|super|win`; Display prints `Meta-` | Works on macOS but is unidiomatic | Decided: Tabby's macOS defaults, for the actions Fogu has (`Action`, `action.rs:50-126`). (a) `keychord.rs:157-160`: accept `"cmd" \| "command"` as `meta` (iced maps ⌘ to `Modifiers::LOGO`, `keys.rs:45`) and `"option" \| "opt"` as `alt` (today only `alt`). (b) `default_keymap()` returns `default_keymap_macos()` when `cfg!(target_os = "macos")`, else `default_keymap_linux()`; tests call each table by name. (c) `Display` prints `Cmd-` for `meta` and `Option-` for `alt` when `cfg!(target_os = "macos")`. (d) Bindings, from Tabby's two default files: Cmd-T new tab, Cmd-W close tab, Cmd-R rename, Ctrl-Tab / Ctrl-Shift-Tab next/previous, Cmd-1..9 tab N (tab 10 unbound), Cmd-D split **down**, Cmd-Shift-D split **right**, Cmd-Option-arrows pane focus, Cmd-Option-Enter maximize pane, Cmd-Shift-W close pane, Cmd-E profile picker, Ctrl-Cmd-F fullscreen, Cmd-C/V copy/paste, Cmd-A select all, Cmd-K clear, Cmd-= / Cmd-Shift-= zoom in, Cmd-- / Cmd-Shift-- zoom out, Cmd-0 reset zoom, Option-PageUp/PageDown page scroll, Shift-PageUp/PageDown scroll top/bottom. Fogu-only actions keep their Linux chord with Ctrl-Shift → Cmd-Shift (Cmd-Shift-O SFTP panel, Cmd-Shift-M profile manager); everything unbound on Linux stays unbound. Tabby actions Fogu has no `Action` for, **deferred** (not implemented in this plan): reopen-tab, move-tab-left/right, command-selector, switch-profile, explode/combine-tabs, pane-nav-previous/next, rearrange-panes, toggle-last-tab, search, pane-focus-all, focus-all-tabs, switch-meta-option, scroll-up/down by line, and the line-editing chords (home/end/word/delete-line/insert-new-line), which Fogu forwards to the shell as keys. (e) README table gets a macOS column. Fix the `keychord.rs:1-2` doc. (f) Existing tests assert the Linux table through `default_keymap()`/`Config::default()` (`action.rs:423-428`, `config/mod.rs:308-316`, `ui/keys.rs:251-252`): point them at `default_keymap_linux()` explicitly, add one `default_keymap_macos` test (Cmd-T → NewTab, Cmd-D → SplitDown), and make the `Config::default()` assertion pick its chord by `cfg!`. `user_hotkeys_merge_over_the_defaults` (`action.rs:410-421`) goes through `Deserialize`, which merges over the *platform* table (`:394`), and overrides `Ctrl-Shift-T` expecting NewTab to end up empty: on macOS Cmd-T would stay bound. Make it take NewTab's and CloseTab's first chords from `default_keymap()` (`Ctrl-Shift-T`/`Ctrl-Shift-W` or `Cmd-T`/`Cmd-W`; the CloseTab lookup is at `:418`) and assert on those, so the same test holds on both. |
| 15 | U | `crates/ui/src/terminal_view.rs:112-114` (`control()` for link hover), `crates/ui/src/message.rs:171-173` | Ctrl+click opens links | macOS convention is Cmd+click | `modifiers.command()` (`iced_core-0.14.0/src/keyboard/modifiers.rs:80-88`: ⌘ on macOS, Ctrl elsewhere); doc comment says "Ctrl+click (Cmd+click on macOS)". `mouse.rs:178` (Alt/Ctrl = block selection) stays. |
| 16 | F | `crates/ui/src/metrics.rs:198-201` `resolve`, `:263-290` `find_face`; defaults `"monospace"` at `metrics.rs:27` and `crates/core/src/config/mod.rs:109` | `monospace` resolves through fontdb's generic family, else the first regular monospaced face in database order | cosmic-text sets the generic monospace family to `Noto Sans Mono` (`cosmic-text-0.15.0/src/font/system.rs:158`), which stock macOS lacks: an arbitrary monospaced face is picked (Andale Mono, Courier, PT Mono...) with a warning | In `resolve`, before the lookups: `if cfg!(target_os = "macos") { system.raw().db_mut().set_monospace_family("Menlo"); }`. One line; the config default stays `monospace` on both OSes, so configs remain portable. UI font Inter is bundled (`crates/ui/src/lib.rs:77-81`): fine. |
| 17 | U | `crates/core/src/config/io.rs:58-65` `config_dir` (`xdg` crate), `crates/app/src/main.rs:131-140` `state_dir`, `Cargo.toml:74-78` | `~/.config/fogu/config.toml`, `~/.local/state/fogu/` | `xdg` compiles on any unix (`xdg-3.0.0/src/lib.rs:1`): works, just not where Mac users look | Open question C. Recommended: no change (zero code, dotfile-friendly, README already documents the path, `README.md:115`). Revisit with the `.app`. Only the `Cargo.toml:74-78` "Linux-only" comment needs rewording. |
| 18 | F | `crates/session/src/ssh/x11.rs:38-50` `Display::parse`, `:284-285` | X11 forwarding connects to `/tmp/.X11-unix/X{n}` | XQuartz sets `DISPLAY=/private/tmp/com.apple.launchd.XXXX/org.xquartz:0`; the socket file name includes the `:0`. `rsplit_once(':')` (`:39`) strips it and treats the rest as a TCP host, `is_local()` (`:57-59`) is false so no `.Xauthority` cookie matches (`:119-120`), and XQuartz does not create `/tmp/.X11-unix/X0`. Forwarding fails with "cannot connect". | In `parse`, when `text` starts with `/`, keep the whole string as the socket path (new `socket: Option<String>` field, `host` empty so `is_local()` is true; OpenSSH keeps the full path too, `channels.c` `x11_connect_display`); at `:284-285` connect to `socket` when set, else `/tmp/.X11-unix/X{n}`. Tests: parse of the XQuartz form, and cookie selection picks the `FAMILY_LOCAL` entry for it. Niche; phase 4. |
| 19 | U | `crates/core/src/ssh_config/profiles.rs:203` (`_ => self.note(&d.name)`) | Unknown `~/.ssh/config` keywords become import notes | Nearly every macOS config has `UseKeychain yes` and `AddKeysToAgent yes` (Apple's OpenSSH): two noise notes per import | Add `usekeychain` and `addkeystoagent` to the silently ignored keywords (two lines, no cfg). |
| 20 | P | `.github/workflows/ci.yml:16-41`, `scripts/check.sh` | One `ubuntu-24.04` job | No macOS coverage; a Linux-only PR can break the mac build | Phase 3. See there. |
| 21 | P | `README.md:8,14,99,111,192`, `crates/core/src/lib.rs:3`, `Cargo.toml:74-78,98-102,105-107` comments, `crates/ui/src/lib.rs:69-71` | "Linux-only" statements | Stale once macOS ships | Update in phase 3. |
| 22 | F | `crates/ui/src/app.rs:329-332` (`Message::CloseRequested` → `save_on_exit`), `crates/ui/src/recovery.rs:247-262,330-336` | The final recovery save runs on the window's close request | winit installs an app menu whose Quit item (Cmd-Q, Dock "Quit") calls `terminate:` (`winit-0.30.13/src/platform_impl/macos/menu.rs:66-72`); `applicationWillTerminate` calls `window.close()` on every window (`app_state.rs:166-172`, `event_loop.rs:430-434`), which emits `Destroyed`, not `CloseRequested` (`window_delegate.rs:144-160`). `save_on_exit` never runs. | Best-effort loss, not data loss: `recovery_tick` already writes the layout every second and scrollback every 30 s (`recovery.rs:247-262`; the writes run on the blocking pool, `:313-317`, so a write in flight at `terminate:` can also be lost), so Cmd-Q behaves like a kill: tabs come back, roughly the last 30 s of scrollback may be missing. Scope decision, not impossibility: the menu's key equivalent wins before the view sees Cmd-Q, and iced 0.14 exposes neither `with_default_menu(false)` nor the app delegate, but `NSApplication::setMainMenu` is a *safe* objc2 API (`objc2-app-kit-0.2.2/src/generated/NSApplication.rs:470`, `sharedApplication(MainThreadMarker::new())` likewise), so a menu without the Quit item plus a Fogu `Quit` action bound to Cmd-Q is possible without `unsafe`; Dock "Quit" still goes through `terminate:`. Not done in this plan (a direct `objc2-app-kit` dependency and a replacement menu for a best-effort loss). Phase 1: document it in the README macOS section; manual check: Cmd-Q, relaunch, tabs restored. |
| 23 | P | `scripts/install.sh:30-33` | `install -Dm755 src dst` (GNU: create parent dirs) | Apple's BSD `install` has `-D` *with an argument* (destdir), so `-Dm755` is parsed as `-D m755`: wrong directory, wrong mode | `mkdir -p "$bin_dir"` then `install -m 755 ...`; same for the icon lines (Linux branch). Works on both. Also tell the user to add `~/.local/bin` to `PATH` or run `~/.local/bin/fogu`: macOS does not put it on `PATH`. |

Checked and fine, no change: `std::os::unix::{fs,process}` extension traits, mode bits and
`CommandExt::exec` (`crates/app/src/cli.rs:34-47`, `crash.rs`, `fsutil.rs`, `recent.rs`,
`config/io.rs`, `sftp/upload.rs`, `agent.rs`; all POSIX; `ICED_BACKEND=tiny-skia` also works on
macOS); `renameat_with(.., NOREPLACE)` (`local_fs.rs:14,185`; rustix maps it to `RENAME_EXCL`
on Apple, `rustix-1.1.5/src/backend/libc/fs/types.rs:536-549`, `src/fs/at.rs:298`); `is_eof`
(`local.rs:229-232`: macOS returns EOF or `EIO`, both paths set `eof = true`; reword the comment);
`/etc/shells` profiles (`builtin.rs:31-74`; macOS ships bash, csh, dash, ksh, sh, tcsh, zsh);
`default_shell` (`local.rs:299-309`: `$SHELL` is set in Finder- and Terminal-launched sessions, the
`/etc/passwd` fallback only matters for odd setups; leave it); agent candidates (`agent.rs:24-41`:
launchd sets `SSH_AUTH_SOCK`, the systemd/gcr paths are harmless no-ops; `agent_tests.rs` are
string tests); `env.rs` stripping (nothing macOS-specific to strip); `signal_number`
(`ssh/shell.rs:224-240`: `USR1` is 30 on macOS, 10 on Linux; cosmetic text only);
`tokio::process::Command::process_group(0)`, `rustix::termios::{tcgetpgrp,tcsetwinsize}`,
`kill_process_group`, `fstatvfs`, `openat`/`statat`/`linkat`/`unlinkat`/`fsync` (all unconditional
or `cfg(not(wasi))` in rustix); `dirs::download_dir`/`home_dir` (`~/Downloads`); `~/.ssh/known_hosts`
and `~/.ssh/config`; `russh`/`russh-cryptovec` (`nix` `mlock`, supports macOS); HiDPI
(`app.rs:246-250`, `view.rs:254-256`: Retina reports 2.0 through the same path; sharpness
**unverified**, manual check in phase 1); `cargo deny` (`deny.toml` has no `[graph].targets`, so
every target's graph is already checked; `RUSTSEC-2024-0436` via `metal` is already ignored).

## Phases

Every manual check below runs twice on the Hackintosh: `fogu` and `fogu --software-render`.
The first cycle also records macOS version, GPU, and whether the display is scaled (HiDPI).

### Phase 1 — compiles and runs on the Hackintosh (~2 h authoring, 2-3 test cycles)

Items 1, 2, 3 (plain decorations), 4 (`None`), 5, 6, 7, 8 (stub), 9, 10, 13, 16, 19, 22 (docs
only), 23.

1. `rustix` `system` feature (`Cargo.toml:52`); `harden_process` and `platform_specific()` gates.
2. `/proc` reads return `None` on macOS; `uname` hostname; `DEFAULT_LOGIN`; private agent
   keep-alive and `$TMPDIR`.
3. `OPENER`, `LAUNCHABLE` + `refuse_open` bundle check, `picker_available`, secrets stub, Menlo,
   Option-as-Meta (with tests), ssh_config keywords, `install.sh` `mkdir -p` + `install -m`.
4. `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run
   --workspace` on Linux before each hand-off; the owner runs the same on the Hackintosh.

Acceptance: `cargo build --release -p fogu` and the test suite pass natively on
`x86_64-apple-darwin` (SSH integration tests may skip, not fail; see phase 3 for the harness);
Linux CI green. Manual (both renderers): window opens, resizes and maximises with the native
title bar; text sharp at the display's scale; zsh is a login shell with Homebrew on `PATH`;
`Option+f` moves a word, `Option+Shift+f` sends `ESC F`, `alt_is_meta = false` types `ƒ`;
Open / Show in file manager / links work and a `.app` is refused; Upload opens `NSOpenPanel`; a
dropped file uploads; a key in the launchd agent authenticates; an RSA key through the private
agent authenticates; Cmd-Q then relaunch restores the tabs (item 22).

Expect the first cycle to turn up build errors invisible from Linux; the second verifies the
checklist; a third if the GPU path misbehaves on the Hackintosh.

### Phase 2 — macOS conventions (~1.5 h authoring, 1-2 test cycles)

Items 12 (after question A), 14 (incl. the test changes in (f)), 15, and optionally the
transparent-title-bar polish of item 3.

Acceptance: the item-14 table works on the Hackintosh (spot-check Cmd-T/W/C/V/A/K/1..9, Cmd-D vs
Cmd-Shift-D, Ctrl-Cmd-F); `Cmd-T` and `Cmd-Option-Left` round-trip through `config.toml`;
`keychord` tests cover `cmd`/`option` parsing and `Cmd-`/`Option-` display; both default tables
have a test; the README table lists both platforms; Cmd+click opens a link; copy-on-select
pastes with Cmd-V and middle click pastes the clipboard.

### Phase 3 — ship: install script, docs, CI (~1 h authoring, 1-2 test cycles)

1. `scripts/install.sh` gets a macOS branch (skip icons and `.desktop`; `~/.local/bin/fogu`;
   `--uninstall` removes it; print the `PATH` hint). README: macOS section (build from source; a
   downloaded binary needs `xattr -d com.apple.quarantine fogu` because it is unsigned; an
   unbundled binary gets a generic Dock icon, not Fogu's, since winit sets the regular
   activation policy for unbundled processes, `app_state.rs:128-131`; Cmd-Q skips the final save, so
   roughly the last 30 s of scrollback and any write in flight may be lost, item 22; the Option dead-key limit, item 13). Item 21 comment updates.
2. `.github/workflows/ci.yml`: a `check-macos` job on `macos-15` (arm64) and `macos-15-intel`
   (both valid GitHub-hosted labels): fmt, clippy `-D warnings`, nextest, doc tests. The fuzz
   manifest check stays Linux-only. macOS runners ship `sshd`, `ssh-keygen`, `ssh-agent`, `nc`
   (no `socat`; `crates/session/tests/sshd/route.rs:128-133` falls back to `nc`). Caveat: the
   harness only skips when a tool is *missing*; if `sshd` is present but fails to start five
   times it panics regardless of `FOGU_REQUIRE_SSHD` (`crates/session/tests/support/sshd.rs:155-189`).
   So the owner runs `cargo nextest run -p fogu-session` on the Hackintosh first; if `sshd` will not
   start unprivileged there, make a start failure a skip unless `FOGU_REQUIRE_SSHD=1` (one
   `if` around the `panic!`), and leave the env var unset on the macOS jobs.
3. No Linux-hosted `--target *-apple-darwin` clippy job: `ring 0.17.14` is in the macOS graph and
   its build script compiles C with `cc` (`ring-0.17.14/build.rs:507-530`), which needs an Apple
   cross toolchain the runner does not have. The native macOS jobs are the cross-check.
4. A tag-triggered `release.yml` (none exists today) that builds both macOS targets and uploads
   `fogu-{aarch64,x86_64}-apple-darwin.tar.gz`. Only if a Linux release workflow is added at the
   same time; otherwise defer and let users build from source.

Acceptance: an ungated `prctl` call fails the macOS CI jobs (compile error); a bare `/proc` read
compiles and silently returns `None` everywhere, so CI does not catch it until phase 4's
positive cwd/program tests exist (item 4); on the Hackintosh
`scripts/install.sh` builds and installs `fogu`, `~/.local/bin/fogu` from Terminal opens a
login shell, and `--uninstall` removes it.

### Phase 4 — native integration (~2 h authoring, 2 test cycles)

Items 4 (`libproc`), 8 (Keychain, after question F), 18 (XQuartz).

Acceptance: a stored SSH password survives a restart and is visible in Keychain Access under
service `fogu`; deleting it there makes Fogu prompt again; two keys with fingerprints sharing
the first 12 characters get two Keychain items; "New tab here" opens in the foreground
process's cwd for a shell without OSC 7; X11 forwarding reaches XQuartz with a cookie;
`cargo deny check` passes with the two target-specific dependencies. Two cycles because
`security-framework` and `libproc` are unverified APIs.

### Deferred — `.app` bundle, signing, notarization

Not planned. When it comes: `scripts/package-macos.sh` (`lipo` universal binary, `Info.plist`,
`fogu.icns` from `assets/fogu.png` via `sips`/`iconutil`), `codesign --options runtime` with a
Developer ID (99 USD/year), `notarytool`, DMG, Homebrew cask, and then question C (paths) and
the minimum macOS version (11.0 is the first with Apple Silicon; wgpu/Metal below 12 is
**unverified**). The Hardened Runtime without `get-task-allow` is also the only anti-attach
mitigation available (item 1).

## Risks

1. **Anti-debugging on macOS is weaker** (item 1): core dumps off, but a same-uid debugger can
   attach to an unsigned binary. The README security section must say so.
2. **Transparent title bar** (item 3 polish) may glitch in iced 0.14 — **unverified**; the
   phase-1 plain title bar has no such risk.
3. **Keymap split**: two default tables, two README columns, configs not portable across OSes
   (a `Cmd-` chord parses on Linux as `Meta-`, which is harmless).
4. **No stored passwords on macOS until phase 4**; the `Disabled` text must say so or users will
   file KWallet bugs from a Mac.
5. **Option-as-Meta vs. layouts that need Option** (`@` on some European layouts): item 13 only
   rewrites ASCII letters to plain `ESC <letter>`, but any Option+key is still Meta-prefixed
   while `alt_is_meta = true` (item 13), so users who type `@`/`[` with Option set
   `alt_is_meta = false`, exactly as Alt users on Linux do. Option dead keys never reach Fogu.
6. **SSH integration tests on macOS**: a present-but-failing `sshd` panics the harness (phase 3
   step 2) until the skip is made explicit; a skipped suite lowers mac coverage silently.
7. **Hackintosh GPU**: Metal on a Hackintosh may differ from real hardware, which is why every
   check runs with and without `--software-render`. A pass there does not prove real-Mac
   rendering; `aarch64` has CI only, nobody runs it.
8. **Cmd-Q skips the final save** (item 22): best effort, roughly the last 1 s of layout / 30 s
   of scrollback, plus any write still in flight. A safe-API menu fix exists if it ever matters.
9. **Test cycles are the critical path**: every phase blocks on the owner building and running on
   the Hackintosh; authoring time is small by comparison.

## Open questions for the owner

- **A. Clipboard on macOS** (item 12): copy-on-select → system clipboard (iTerm2 style,
  recommended) or no copy-on-select at all? Middle click → clipboard paste or nothing?
- **C. Paths** (item 17): keep `~/.config/fogu` + `~/.local/state/fogu` (recommended, zero code)
  or move state/logs under `~/Library`?
- **F. Secrets**: is the Keychain backend (phase 4) required for the first macOS release, or is
  "no stored passwords on macOS" acceptable for a beta?
- **G. Private `ssh-agent`**: keep it on macOS (item 7, two small changes; needed for RSA keys
  since russh refuses them) — recommended — or return `None` there?

## Verification notes

Verified on this machine: every `file:line` above (read directly); dependency resolution for
`aarch64-apple-darwin` via `cargo tree --target`; `cfg` gates in `rustix 1.1.5`, `pty-process
0.5.3`, `rfd 0.17.2`, `oo7 0.6.0`, `xdg 3.0.0`, `window_clipboard 0.5.1`, `winit 0.30.13`,
`iced_core 0.14.0`, `iced_winit 0.14.1`, `cosmic-text 0.15.0` by reading the registry sources;
that Cargo accepts `dep:` for an optional dependency in a target table (scratch crate, cargo
1.99); that iced 0.14 does not expose winit's `OptionAsAlt`, `with_default_menu` or the app
delegate; that `ring 0.17.14` is in the `*-apple-darwin` graph (`cargo tree --target`); Tabby's
macOS defaults (fetched from `tabby-core/src/configDefaults.macos.yaml` and
`tabby-terminal/src/config.ts` on 2026-10-19).

Not verified: any compile or run on macOS; `sshd` behaviour on macOS runners and on the
Hackintosh; AppKit edge resize of a borderless winit window; iced rendering under a transparent
title bar; `rfd` dialogs under winit's run loop at runtime; `security-framework` and `libproc`
APIs (not in the local registry).
