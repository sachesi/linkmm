use std::cell::RefCell;
use std::rc::Rc;

use gio;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::Game;

pub mod mod_list;
pub mod setup_wizard;

pub fn build_ui(app: &libadwaita::Application) {
    let config = Rc::new(RefCell::new(AppConfig::load_or_default()));
    let window = build_main_window(app, Rc::clone(&config));
    window.present();
    if config.borrow().first_run || config.borrow().games.is_empty() {
        setup_wizard::show_setup_wizard(&window, Rc::clone(&config));
    }
}

fn build_main_window(
    app: &libadwaita::Application,
    config: Rc<RefCell<AppConfig>>,
) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Linkmm")
        .default_width(1000)
        .default_height(700)
        .build();

    let split_view = adw::NavigationSplitView::new();

    // --- Sidebar ---
    let sidebar_toolbar_view = adw::ToolbarView::new();
    let sidebar_header = adw::HeaderBar::new();

    // Menu button
    let menu_button = gtk4::MenuButton::new();
    menu_button.set_icon_name("open-menu-symbolic");
    let menu = gio::Menu::new();
    menu.append(Some("Settings"), Some("app.settings"));
    menu.append(Some("About Linkmm"), Some("app.about"));
    menu_button.set_menu_model(Some(&menu));
    sidebar_header.pack_end(&menu_button);
    sidebar_toolbar_view.add_top_bar(&sidebar_header);

    let game_list = gtk4::ListBox::new();
    game_list.add_css_class("navigation-sidebar");
    game_list.set_selection_mode(gtk4::SelectionMode::Single);

    // Content page reference for updating when a game is selected
    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

    {
        let games = config.borrow().games.clone();
        for game in &games {
            let row = build_game_row(game);
            game_list.append(&row);
        }
    }

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    scrolled.set_child(Some(&game_list));

    let sidebar_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    sidebar_box.append(&scrolled);

    let add_game_button = gtk4::Button::with_label("Add Game");
    add_game_button.set_margin_start(8);
    add_game_button.set_margin_end(8);
    add_game_button.set_margin_top(8);
    add_game_button.set_margin_bottom(8);
    sidebar_box.append(&add_game_button);

    sidebar_toolbar_view.set_content(Some(&sidebar_box));

    let sidebar_nav_page = adw::NavigationPage::builder()
        .title("Games")
        .child(&sidebar_toolbar_view)
        .build();

    split_view.set_sidebar(Some(&sidebar_nav_page));

    // --- Content ---
    let content_nav_page = adw::NavigationPage::builder()
        .title("Mods")
        .child(&content_box)
        .build();

    let no_game_status = adw::StatusPage::builder()
        .title("No Game Selected")
        .description("Select a game from the sidebar to manage its mods.")
        .icon_name("applications-games-symbolic")
        .build();
    no_game_status.set_vexpand(true);
    content_box.append(&no_game_status);

    split_view.set_content(Some(&content_nav_page));
    window.set_content(Some(&split_view));

    // Handle game row selection
    {
        let config_clone = Rc::clone(&config);
        let content_box_clone = content_box.clone();
        let content_nav_page_clone = content_nav_page.clone();

        game_list.connect_row_selected(move |_, row| {
            // Remove all existing children from content_box
            while let Some(child) = content_box_clone.first_child() {
                content_box_clone.remove(&child);
            }

            if let Some(row) = row {
                let idx = row.index() as usize;
                let games = config_clone.borrow().games.clone();
                if let Some(game) = games.get(idx) {
                    content_nav_page_clone.set_title(&game.name);
                    let mod_widget = mod_list::build_mod_list(game, Rc::clone(&config_clone));
                    content_box_clone.append(&mod_widget);
                }
            } else {
                content_nav_page_clone.set_title("Mods");
                let status = adw::StatusPage::builder()
                    .title("No Game Selected")
                    .description("Select a game from the sidebar to manage its mods.")
                    .icon_name("applications-games-symbolic")
                    .build();
                status.set_vexpand(true);
                content_box_clone.append(&status);
            }
        });
    }

    // Add Game button handler (re-open wizard)
    {
        let window_clone = window.clone();
        let config_clone = Rc::clone(&config);
        add_game_button.connect_clicked(move |_| {
            setup_wizard::show_setup_wizard(&window_clone, Rc::clone(&config_clone));
        });
    }

    window
}

fn build_game_row(game: &Game) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&game.name)
        .subtitle(game.root_path.to_string_lossy().as_ref())
        .build();
    row
}
