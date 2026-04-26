use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::{GameKind, UmuGameConfig};
use crate::core::steam;

mod fomod;
mod page_app_dir;
mod page_game_select;
mod page_nexus;
mod page_welcome;

pub(crate) use fomod::show_fomod_wizard;
pub use fomod::show_fomod_wizard_from_library;

/// Represents how the user chose to set up a game in the wizard.
#[derive(Clone, Debug)]
pub(super) enum GameSelection {
    /// A Steam-detected or manually-added game (no UMU).
    Steam {
        kind: GameKind,
        app_id: u32,
        path: std::path::PathBuf,
    },
    /// A game configured via UMU-launcher (non-Steam).
    Umu {
        kind: GameKind,
        root_path: std::path::PathBuf,
        umu_cfg: UmuGameConfig,
    },
}

pub fn show_setup_wizard<F: Fn() + 'static>(
    parent: &adw::ApplicationWindow,
    config: Rc<RefCell<AppConfig>>,
    on_finish: F,
) {
    let on_finish_rc: Rc<dyn Fn()> = Rc::new(on_finish);
    let dialog = adw::Window::builder()
        .title("Linkmm Setup")
        .modal(true)
        .transient_for(parent)
        .default_width(600)
        .default_height(500)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let stack = gtk4::Stack::new();
    stack.set_vexpand(true);
    stack.set_transition_type(gtk4::StackTransitionType::SlideLeft);

    // --- Page 1: Welcome ---
    let welcome_page = page_welcome::build_welcome_page();
    stack.add_named(&welcome_page, Some("welcome"));

    // --- Page 2: Select Game ---
    let detected_games = steam::detect_games();
    let selected_game: Rc<RefCell<Option<GameSelection>>> = Rc::new(RefCell::new(None));

    let (game_page, selected_game_clone) =
        page_game_select::build_game_select_page(&stack, detected_games, Rc::clone(&selected_game));
    stack.add_named(&game_page, Some("select_game"));

    // --- Page 3: App Directory ---
    let selected_app_dir: Rc<RefCell<Option<std::path::PathBuf>>> = Rc::new(RefCell::new(None));

    let app_dir_page = page_app_dir::build_app_dir_page(&stack, Rc::clone(&selected_app_dir));
    stack.add_named(&app_dir_page, Some("app_dir"));

    // --- Page 4: NexusMods API Key ---
    let nexus_page = page_nexus::build_nexus_page(
        &dialog,
        Rc::clone(&config),
        Rc::clone(&selected_game_clone),
        Rc::clone(&selected_app_dir),
        Rc::clone(&on_finish_rc),
    );
    stack.add_named(&nexus_page, Some("nexus_key"));

    toolbar_view.set_content(Some(&stack));
    dialog.set_content(Some(&toolbar_view));

    // Wire up "Get Started" button on page 1
    {
        let stack_clone = stack.clone();
        let get_started = welcome_page
            .last_child()
            .and_then(|w| w.last_child())
            .and_then(|w| w.downcast::<gtk4::Button>().ok());
        if let Some(btn) = get_started {
            btn.connect_clicked(move |_| {
                stack_clone.set_visible_child_name("select_game");
            });
        }
    }

    dialog.present();
}
