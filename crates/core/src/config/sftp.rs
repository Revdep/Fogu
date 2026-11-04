//! `[sftp]`: the SFTP side panel and its transfer engine. The engine fields mirror
//! `fogu_session::sftp::SftpOptions` (same defaults; the UI maps one onto the other).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// SFTP panel and transfer settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a flat, directly-TOML-mapped settings record, not a state machine"
)]
pub struct SftpCfg {
    /// Where double-click downloads go; `None` = the XDG download directory (`~/Downloads`).
    pub download_dir: Option<PathBuf>,
    /// Show dot files when the panel opens.
    pub show_hidden: bool,
    /// Panel width as a share of the tab (0.15–0.85).
    pub panel_width: f32,
    /// Start with "follow terminal" on: the panel goes where the remote shell's cwd is (needs
    /// OSC 7 from the shell); manual navigation turns it off.
    pub follow_cwd: bool,
    /// Seconds a finished transfer stays in the transfers list.
    pub transfer_linger_secs: u64,
    /// Files transferred at the same time per connection (at least 1).
    pub max_parallel_files: usize,
    /// Keep `<name>.part` when a download is cancelled or fails.
    pub keep_partial: bool,
    /// Seconds one SFTP request may wait for its reply.
    pub request_timeout_secs: u64,
    /// Entries shown per directory; the rest are dropped with a notice.
    pub max_listing_entries: usize,
    /// Entries one recursive transfer may walk before it fails.
    pub max_walk_entries: usize,
    /// Directory depth one recursive transfer may walk before it fails.
    pub max_walk_depth: usize,
    /// Let a download continue past the size the server reported (files growing while being
    /// fetched). Off by default: a server sending more than it announced is stopped.
    pub allow_growing_files: bool,
    /// Hard limit per downloaded file in bytes (`None` = no limit beyond the reported size).
    pub max_download_bytes: Option<u64>,
    /// Free space a download must leave on the local disk, in bytes.
    pub free_space_reserve_bytes: u64,
    /// Keep the server's executable bit on downloaded files (`0755`). Off: downloads are
    /// always `0644`, so a `.desktop` file, script or `AppImage` from the server cannot be run
    /// by a double-click in a file manager.
    pub keep_executable_bit: bool,
    /// Linux/BSD under Wayland: start Fogu on `XWayland`, where files can be dropped onto the
    /// panel (winit 0.30 has no Wayland drops). Off: native Wayland, no drops. Read at start-up.
    pub wayland_drag_and_drop: bool,
}

impl Default for SftpCfg {
    fn default() -> Self {
        Self {
            download_dir: None,
            show_hidden: false,
            panel_width: 0.4,
            follow_cwd: true,
            transfer_linger_secs: 10,
            max_parallel_files: 4,
            keep_partial: false,
            request_timeout_secs: 60,
            max_listing_entries: 100_000,
            max_walk_entries: 200_000,
            max_walk_depth: 64,
            allow_growing_files: false,
            max_download_bytes: None,
            free_space_reserve_bytes: 1 << 30,
            keep_executable_bit: false,
            wayland_drag_and_drop: false,
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn sftp_section_overrides_and_defaults() {
        let text = "[sftp]\nmax_download_bytes = 1048576\nshow_hidden = true\n";
        let config: crate::Config = toml::from_str(text).unwrap();
        assert_eq!(config.sftp.max_download_bytes, Some(1 << 20));
        assert!(config.sftp.show_hidden);
        assert_eq!(config.sftp.free_space_reserve_bytes, 1 << 30);
        assert_eq!(config.sftp.max_parallel_files, 4);
    }
}
