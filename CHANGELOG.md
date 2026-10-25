# Changelog

## 0.1.7 (2026-10-25)

<!-- fogu-update: {"schema":1,"from":">=0.1.6, <0.2.0"} -->

### Added
- Settings → Updates: Fogu looks for a new release, shows what changed and installs it only when
  you choose, checking its signature first; then it restarts with your tabs. A new version is
  announced at most twice. Linux for now; macOS and Windows link to the download. Needs `curl`.
- Profile menu → "Use for new tabs": new tabs open that profile (stored or built-in); the sidebar
  marks it "default". The built-in "Default shell" is now named after the computer.
- Settings → config.toml: the whole file as text, for the keys the pages don't show. Save writes it
  only when it loads (the error otherwise) and applies it at once; Revert reads it again.
- Colour schemes: Catppuccin Latte, Frappé and Macchiato next to Mocha; Gruvbox Dark and Light;
  Nord; Tokyo Night, Storm, Moon and Day; One Dark.
- Settings → SFTP → *Enable drag & drop on Wayland* (`[sftp] wayland_drag_and_drop`, off by
  default): Fogu opens on XWayland, where files can be dropped onto the SFTP panel; local shells
  and the apps it opens keep `WAYLAND_DISPLAY` (`wl-copy`, Wayland apps). Read at start-up. Under
  XWayland, on HiDPI screens it may look small or blurry, KDE's window blur is off and input
  methods need `XMODIFIERS`. By Nicola ([@Oginexe](https://github.com/Oginexe)) in
  [#11](https://github.com/Revdep/Fogu/pull/11). Thank you!

### Changed
- The Linux release runs on glibc 2.28 or newer (was 2.35): Debian 10, Ubuntu 20.04, RHEL 8.
- Nocturne is the default colour scheme, with new terminal colours, and Fogu Light is a new light
  palette; both are Fogu's own. Fogu Dark is gone: a config that names it gets Nocturne.

### Security
- Updated crypto-bigint to 0.7.6 to fix timing variability in integer operations used by SSH cryptography.

## 0.1.6 (2026-10-23)

### Added
- Windows support (alpha): ConPTY shells, Credential Manager, the OpenSSH agent pipe and Windows
  SFTP paths, by [@Federicokalik](https://github.com/Federicokalik) in
  [#3](https://github.com/Revdep/Fogu/pull/3). Thank you!
- SSH tabs can keep their history after a restart (off by default, Settings → Tab recovery). It is
  saved only encrypted, with a key kept in your system's password vault.
- A thin terminal scrollbar you can drag through the history; a click on its track jumps there.
- Settings → Appearance → Window → Rounded corners: panes, side panels and pages as rounded
  tiles (on by default), now also for a single pane, a see-through window and a colour tab
  background.

### Security
- Windows: network (UNC) paths in links and in the working directory a program reports are never
  touched, so they can't leak your Windows password hash.
- Windows: Fogu's files get an access list for you and SYSTEM only; an SSH agent pipe created by
  another user is refused.
- Windows: SFTP downloads refuse every reserved device name; profiles whose user or host differ
  only in case no longer share a stored password (stored secrets move to the new names by
  themselves).
- Pasted text can't end a bracketed paste with an 8-bit control character.
- Tab titles drop bidirectional override characters, so a title can't read reversed.
- Passwords read from the macOS Keychain and Windows Credential Manager leave no unzeroed copy.
- Saving the config can no longer lose it on a crash, and a symlinked `config.toml` stays a link.
- Release archives carry every licence and a GitHub build attestation; build downloads are
  checked against their hashes.

### Changed
- The Linux binary is 34% smaller (29.3 → 19.4 MB), opens its first frame faster, uses about
  9% less memory and 21 fewer threads, with the same terminal throughput.
- One log file, `fogu.log`, rotated at start-up past 4 MB (it replaces the daily files).
- Clearer errors: failed saves name the file.
- Tab headers shrink to fit before the tab bar scrolls, and the bar always keeps room to grab and
  move the window.
- Thinner scrollbars everywhere; menus and dialogs get a dark ring on dark themes.

### Fixed
- Window opacity on Linux (Mesa) works in small steps instead of jumping between opaque and two
  thirds.
- Trackpad scrolling is as fast as the mouse wheel.
- Right-clicking a tab no longer scrolls the tab bar back to the start.
- The terminal keeps the same margin on all four sides.
