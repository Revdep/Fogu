//! The "file exists" dialog and its answers: Overwrite, Skip, Keep both (`name (1)`), and
//! Resume when the engine found the existing partial file plausible.

use fogu_session::SftpRequest;
use fogu_session::sftp::TransferId;
use iced::Element;
use iced::widget::{button, checkbox, column, row, text};
use tokio::sync::mpsc::Sender;

use super::panel::msg;
use super::transfers::{self, Conflict, ConflictChoice, Direction, Origin, conflict_text};
use super::{PanelMsg, SftpPanelState, transfer_label};
use crate::message::Message;
use crate::modal;
use crate::notifications::{Level, Notification};
use crate::tab::TabId;

impl SftpPanelState {
    /// Answers the current conflict; re-issues the chosen files as new transfers. Returns notes
    /// for names that could not be re-issued.
    pub fn answer_conflict(
        &mut self,
        tx: &Sender<SftpRequest>,
        choice: ConflictChoice,
    ) -> Vec<Notification> {
        let chosen = self.conflicts.answer(choice);
        let Some(policy) = choice.reissue_as() else {
            return Vec::new();
        };
        let mut by_transfer: Vec<(TransferId, Vec<String>)> = Vec::new();
        for Conflict { transfer, path, .. } in chosen {
            match by_transfer.iter_mut().find(|(t, _)| *t == transfer) {
                Some((_, paths)) => paths.push(path),
                None => by_transfer.push((transfer, vec![path])),
            }
        }
        let title = format!("Not re-issued ({})", choice.label());
        let mut notes = Vec::new();
        for (transfer, paths) in by_transfer {
            let Some(old) = self.transfers.get(transfer) else {
                continue;
            };
            let (origin, direction) = (old.origin.clone(), old.direction);
            let open_when_done = old.open_when_done;
            let (cmds, rejected) = transfers::reissue(&origin, &paths, policy);
            if !rejected.is_empty() {
                notes.push(
                    Notification::new(Level::Warn, &title)
                        .detail(format!("Invalid name(s): {}", rejected.join(", "))),
                );
            }
            for cmd in cmds {
                if let (
                    fogu_session::SftpCmd::Download { local_dir, .. },
                    Origin::Download {
                        local_dir: base, ..
                    },
                ) = (&cmd, &origin)
                    && !transfers::real_subdir(base, local_dir)
                {
                    notes.push(
                        Notification::new(Level::Warn, &title)
                            .detail("A folder on the way is missing or is a symbolic link."),
                    );
                    continue;
                }
                let label = format!("{} ({})", transfer_label(&cmd), choice.label());
                match self.issue(tx, cmd, direction, label, origin.clone()) {
                    Ok(row) => row.open_when_done = open_when_done,
                    Err(err) => notes.push(Notification::new(Level::Error, "SFTP").detail(err)),
                }
            }
        }
        notes
    }
}

/// The "file exists" dialog for the panel's first queued conflict.
pub fn conflict_dialog(tab: TabId, panel: &SftpPanelState) -> Option<Element<'_, Message>> {
    let conflict = panel.conflicts.current()?;
    let direction = panel
        .transfers
        .get(conflict.transfer)
        .map_or(Direction::Download, |t| t.direction);
    let more = panel.conflicts.queue.len() - 1;
    let choose = |label: &'static str, choice| {
        button(text(label))
            .style(crate::style::secondary)
            .on_press(msg(tab, PanelMsg::Conflict(choice)))
    };
    let mut buttons = row![].spacing(8);
    if conflict.resumable {
        buttons = buttons.push(choose("Resume", ConflictChoice::Resume));
    }
    buttons = buttons
        .push(choose("Overwrite", ConflictChoice::Overwrite))
        .push(choose("Keep both", ConflictChoice::KeepBoth))
        .push(choose("Skip", ConflictChoice::Skip));
    let mut content = column![
        text("File exists").size(18),
        text(conflict_text(conflict, direction)),
    ]
    .spacing(12);
    if conflict.resumable {
        content = content.push(
            text("It looks like an interrupted transfer: Resume copies only the rest.").size(13),
        );
    }
    if more > 0 {
        content = content.push(
            checkbox(panel.conflicts.apply_all)
                .label(format!("Apply to all ({more} more)"))
                .on_toggle(move |on| msg(tab, PanelMsg::ConflictApplyAll(on))),
        );
    }
    content = content.push(buttons);
    Some(modal::frame(content.into(), 460.0))
}
