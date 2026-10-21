//! The search bar (`Action::Find`): plain-text search of a pane's screen and scrollback. Each
//! match becomes the selection, so it shows highlighted and copies like any other selection.

use iced::widget::{Id, button, container, operation, row, text, text_input};
use iced::{Element, Length, Task};

use crate::app::App;
use crate::context_menu::panel_style;
use crate::icons::{Icon, icon};
use crate::message::Message;
use crate::palette::Look;
use crate::tab::{PaneId, Tab};
use crate::tab_bar::hover_style;

const SEARCH_INPUT: &str = "fogu-search-input";

/// The open search bar.
#[derive(Debug)]
pub struct Search {
    /// The pane being searched.
    pub pane: PaneId,
    pub query: String,
    /// The last search found a match.
    found: bool,
}

/// Search bar input.
#[derive(Clone, Debug)]
pub enum SearchMsg {
    Input(String),
    /// Enter: the next match up (older), or down with Shift.
    Submit,
    /// A search arrow button: up (`true`) or down.
    Step(bool),
    Close,
}

impl App {
    /// Opens the bar over the focused pane (keeping the query if it is already open) and focuses
    /// and selects its field.
    pub(crate) fn start_search(&mut self) -> Task<Message> {
        let Some(pane) = self.active_pane().map(|p| p.id) else {
            return Task::none();
        };
        let query = self.search.take().map(|s| s.query).unwrap_or_default();
        self.search = Some(Search {
            pane,
            query,
            found: true,
        });
        let id = Id::new(SEARCH_INPUT);
        Task::batch([operation::focus(id.clone()), operation::select_all(id)])
    }

    /// The open bar, when its pane is in `tab`.
    pub(crate) fn search_in(&self, tab: &Tab) -> Option<&Search> {
        self.search
            .as_ref()
            .filter(|s| tab.panes.contains_key(&s.pane))
    }

    pub(crate) fn on_search(&mut self, message: SearchMsg) -> Task<Message> {
        let Some(search) = self.search.as_mut() else {
            return Task::none();
        };
        let (restart, up) = match message {
            SearchMsg::Input(query) => {
                search.query = query;
                (true, true)
            }
            SearchMsg::Submit => (false, !self.modifiers.shift()),
            SearchMsg::Step(up) => (false, up),
            SearchMsg::Close => {
                self.search = None;
                return Task::none();
            }
        };
        let (id, query) = (search.pane, search.query.clone());
        let Some(pane) = self.pane_mut(id) else {
            return Task::none();
        };
        // Typing restarts from the cursor rather than from the previous match.
        if restart {
            pane.term.clear_selection();
        }
        let found = query.is_empty() || pane.term.search(&query, up);
        pane.refresh();
        if let Some(search) = self.search.as_mut() {
            search.found = found;
        }
        Task::none()
    }
}

/// `panes` with the bar at their top right while `search` is open.
pub fn over<'a>(
    panes: Element<'a, Message>,
    search: Option<&'a Search>,
    look: Look,
) -> Element<'a, Message> {
    match search {
        Some(search) => iced::widget::stack![panes, view(search, look)].into(),
        None => panes,
    }
}

fn view(search: &Search, look: Look) -> Element<'_, Message> {
    let arrow = |label, up| {
        button(text(label).size(13).color(look.fg))
            .padding([2, 8])
            .style(move |_theme, status| hover_style(look, status, false))
            .on_press(Message::Search(SearchMsg::Step(up)))
    };
    let field = text_input("Find", &search.query)
        .id(Id::new(SEARCH_INPUT))
        .on_input(|q| Message::Search(SearchMsg::Input(q)))
        .on_submit(Message::Search(SearchMsg::Submit))
        .size(13)
        .padding(4)
        .width(220);
    let mut bar = row![field].spacing(4).align_y(iced::Alignment::Center);
    if !search.found {
        bar = bar.push(text("No matches").size(12).color(look.danger));
    }
    // `icon` fills the space it is given: a fixed size keeps the bar small.
    let close = button(icon(Icon::Close, 13.0, look.fg))
        .width(22)
        .height(22)
        .padding(0)
        .style(move |_theme, status| hover_style(look, status, false))
        .on_press(Message::Search(SearchMsg::Close));
    let bar = bar
        .push(arrow("↑", true))
        .push(arrow("↓", false))
        .push(close);
    let panel = container(bar)
        .padding(6)
        .style(move |_theme: &iced::Theme| panel_style(look));
    container(panel)
        .padding(8)
        .width(Length::Fill)
        .align_right(Length::Fill)
        .into()
}
