use std::cell::RefCell;
use std::rc::Rc;

use gio;
use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::{Game, GameKind};
use crate::core::steam;

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
    let welcome_page = build_welcome_page();
    stack.add_named(&welcome_page, Some("welcome"));

    // --- Page 2: Select Game ---
    let detected_games = steam::detect_games();
    let selected_game: Rc<RefCell<Option<(GameKind, std::path::PathBuf)>>> =
        Rc::new(RefCell::new(None));

    let (game_page, selected_game_clone) =
        build_game_select_page(&stack, detected_games, Rc::clone(&selected_game));
    stack.add_named(&game_page, Some("select_game"));

    // --- Page 3: NexusMods API Key ---
    let nexus_page = build_nexus_page(
        &dialog,
        Rc::clone(&config),
        Rc::clone(&selected_game_clone),
        Rc::clone(&on_finish_rc),
    );
    stack.add_named(&nexus_page, Some("nexus_key"));

    toolbar_view.set_content(Some(&stack));
    dialog.set_content(Some(&toolbar_view));

    // Wire up "Get Started" button on page 1
    {
        let stack_clone = stack.clone();
        let get_started = welcome_page
            .last_child() // The button box
            .and_then(|w| w.last_child()) // The button
            .and_then(|w| w.downcast::<gtk4::Button>().ok());
        if let Some(btn) = get_started {
            btn.connect_clicked(move |_| {
                stack_clone.set_visible_child_name("select_game");
            });
        }
    }

    dialog.present();
}

fn build_welcome_page() -> gtk4::Box {
    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    page.set_vexpand(true);

    let status = adw::StatusPage::builder()
        .icon_name("applications-games-symbolic")
        .title("Welcome to Linkmm")
        .description(
            "The link-based mod manager for Bethesda games.\n\nLet\u{2019}s set up your first game.",
        )
        .build();
    status.set_vexpand(true);
    page.append(&status);

    let button_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    button_box.set_halign(gtk4::Align::Center);
    button_box.set_margin_bottom(24);

    let get_started_btn = gtk4::Button::with_label("Get Started");
    get_started_btn.add_css_class("suggested-action");
    get_started_btn.add_css_class("pill");
    button_box.append(&get_started_btn);

    page.append(&button_box);
    page
}

fn build_game_select_page(
    stack: &gtk4::Stack,
    detected_games: Vec<(GameKind, std::path::PathBuf)>,
    selected_game: Rc<RefCell<Option<(GameKind, std::path::PathBuf)>>>,
) -> (
    gtk4::Box,
    Rc<RefCell<Option<(GameKind, std::path::PathBuf)>>>,
) {
    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    page.set_vexpand(true);
    page.set_margin_start(24);
    page.set_margin_end(24);
    page.set_margin_top(24);
    page.set_margin_bottom(24);

    let header_label = gtk4::Label::new(Some("Select Your Game"));
    header_label.add_css_class("title-1");
    header_label.set_halign(gtk4::Align::Start);
    page.append(&header_label);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);

    let inner_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);

    if detected_games.is_empty() {
        let no_games = adw::StatusPage::builder()
            .title("No Games Found Automatically")
            .description("No Steam games were detected. Add your game manually.")
            .icon_name("edit-find-symbolic")
            .build();
        inner_box.append(&no_games);
    } else {
        let section_label = gtk4::Label::new(Some("Auto-Detected Games"));
        section_label.add_css_class("heading");
        section_label.set_halign(gtk4::Align::Start);
        inner_box.append(&section_label);

        let game_list = gtk4::ListBox::new();
        game_list.add_css_class("boxed-list");
        game_list.set_selection_mode(gtk4::SelectionMode::None);

        let check_buttons: Rc<RefCell<Vec<gtk4::CheckButton>>> = Rc::new(RefCell::new(Vec::new()));

        for (kind, path) in &detected_games {
            let row = adw::ActionRow::builder()
                .title(kind.display_name())
                .subtitle(path.to_string_lossy().as_ref())
                .build();

            let check = gtk4::CheckButton::new();
            row.add_prefix(&check);
            row.set_activatable_widget(Some(&check));

            check_buttons.borrow_mut().push(check.clone());

            let kind_clone = kind.clone();
            let path_clone = path.clone();
            let selected_game_clone = Rc::clone(&selected_game);
            let all_checks = Rc::clone(&check_buttons);

            check.connect_toggled(move |this_check| {
                if this_check.is_active() {
                    // Uncheck all other buttons
                    for btn in all_checks.borrow().iter() {
                        if btn != this_check {
                            btn.set_active(false);
                        }
                    }
                    *selected_game_clone.borrow_mut() =
                        Some((kind_clone.clone(), path_clone.clone()));
                } else {
                    let mut sel = selected_game_clone.borrow_mut();
                    if sel.as_ref().map(|(k, _)| k == &kind_clone).unwrap_or(false) {
                        *sel = None;
                    }
                }
            });

            game_list.append(&row);
        }

        inner_box.append(&game_list);
    }

    scrolled.set_child(Some(&inner_box));
    page.append(&scrolled);

    // "Add Game Manually" button
    let add_manual_btn = gtk4::Button::with_label("Add Game Manually");
    add_manual_btn.set_halign(gtk4::Align::Center);

    let selected_game_for_manual = Rc::clone(&selected_game);
    add_manual_btn.connect_clicked(move |btn| {
        show_add_game_dialog(btn, Rc::clone(&selected_game_for_manual));
    });
    page.append(&add_manual_btn);

    // Navigation buttons
    let nav_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    nav_box.set_halign(gtk4::Align::End);
    nav_box.set_margin_top(8);

    let next_btn = gtk4::Button::with_label("Next");
    next_btn.add_css_class("suggested-action");

    nav_box.append(&next_btn);
    page.append(&nav_box);

    let stack_clone = stack.clone();
    next_btn.connect_clicked(move |_| {
        stack_clone.set_visible_child_name("nexus_key");
    });

    (page, selected_game)
}

fn show_add_game_dialog(
    parent_widget: &gtk4::Button,
    selected_game: Rc<RefCell<Option<(GameKind, std::path::PathBuf)>>>,
) {
    let parent_window = parent_widget
        .root()
        .and_then(|r| r.downcast::<gtk4::Window>().ok());

    let dialog = adw::Window::builder()
        .title("Add Game Manually")
        .modal(true)
        .default_width(480)
        .default_height(360)
        .build();

    if let Some(pw) = &parent_window {
        dialog.set_transient_for(Some(pw));
    }

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content_box.set_margin_start(24);
    content_box.set_margin_end(24);
    content_box.set_margin_top(12);
    content_box.set_margin_bottom(12);

    // Game kind selector
    let kind_group = adw::PreferencesGroup::builder().title("Game").build();

    let game_strings: Vec<String> = GameKind::all()
        .iter()
        .map(|k| k.display_name().to_string())
        .collect();
    let game_strings_ref: Vec<&str> = game_strings.iter().map(|s| s.as_str()).collect();

    let kind_row = adw::ComboRow::builder().title("Game").build();
    let model = gtk4::StringList::new(&game_strings_ref);
    kind_row.set_model(Some(&model));
    kind_group.add(&kind_row);
    content_box.append(&kind_group);

    // Root path
    let path_group = adw::PreferencesGroup::builder().title("Paths").build();

    let root_path_row = adw::EntryRow::builder().title("Root Path").build();
    let browse_root_btn = gtk4::Button::new();
    browse_root_btn.set_icon_name("folder-open-symbolic");
    browse_root_btn.set_valign(gtk4::Align::Center);
    root_path_row.add_suffix(&browse_root_btn);

    path_group.add(&root_path_row);
    content_box.append(&path_group);

    // Browse button for root path
    {
        let root_path_row_clone = root_path_row.clone();
        let parent_window_clone = parent_window.clone();
        browse_root_btn.connect_clicked(move |_| {
            let file_dialog = gtk4::FileDialog::new();
            file_dialog.set_title("Select Game Root Folder");
            let row_clone = root_path_row_clone.clone();
            file_dialog.select_folder(
                parent_window_clone.as_ref(),
                None::<&gio::Cancellable>,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            row_clone.set_text(&path.to_string_lossy());
                        }
                    }
                },
            );
        });
    }

    // Buttons
    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk4::Align::End);
    btn_box.set_margin_top(12);

    let cancel_btn = gtk4::Button::with_label("Cancel");
    let add_btn = gtk4::Button::with_label("Add");
    add_btn.add_css_class("suggested-action");

    btn_box.append(&cancel_btn);
    btn_box.append(&add_btn);
    content_box.append(&btn_box);

    toolbar_view.set_content(Some(&content_box));
    dialog.set_content(Some(&toolbar_view));

    let dialog_clone = dialog.clone();
    cancel_btn.connect_clicked(move |_| {
        dialog_clone.destroy();
    });

    let dialog_clone2 = dialog.clone();
    let kind_row_clone = kind_row.clone();
    let root_path_row_clone = root_path_row.clone();
    add_btn.connect_clicked(move |_| {
        let root_text = root_path_row_clone.text().to_string();
        if root_text.is_empty() {
            return;
        }
        let root_path = std::path::PathBuf::from(&root_text);
        let kind_idx = kind_row_clone.selected() as usize;
        let all_kinds = GameKind::all();
        if let Some(kind) = all_kinds.get(kind_idx) {
            *selected_game.borrow_mut() = Some((kind.clone(), root_path));
        }
        dialog_clone2.destroy();
    });

    dialog.present();
}

fn build_nexus_page(
    wizard_window: &adw::Window,
    config: Rc<RefCell<AppConfig>>,
    selected_game: Rc<RefCell<Option<(GameKind, std::path::PathBuf)>>>,
    on_finish: Rc<dyn Fn()>,
) -> gtk4::Box {
    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    page.set_vexpand(true);
    page.set_margin_start(24);
    page.set_margin_end(24);
    page.set_margin_top(24);
    page.set_margin_bottom(24);

    let header_label = gtk4::Label::new(Some("NexusMods API Key"));
    header_label.add_css_class("title-1");
    header_label.set_halign(gtk4::Align::Start);
    page.append(&header_label);

    let desc_label = gtk4::Label::new(Some(
        "Enter your NexusMods API key to browse and download mods.",
    ));
    desc_label.set_wrap(true);
    desc_label.set_halign(gtk4::Align::Start);
    page.append(&desc_label);

    let link_btn = gtk4::LinkButton::builder()
        .label("Get your API key on NexusMods")
        .uri("https://www.nexusmods.com/users/myaccount?tab=api+access")
        .halign(gtk4::Align::Start)
        .build();
    page.append(&link_btn);

    let prefs_group = adw::PreferencesGroup::new();
    // PasswordEntryRow (libadwaita 1.2+) includes a built-in visibility toggle
    let api_key_row = adw::PasswordEntryRow::builder().title("API Key").build();

    prefs_group.add(&api_key_row);
    page.append(&prefs_group);

    // Validate button
    let validate_btn = gtk4::Button::with_label("Validate Key");
    validate_btn.set_halign(gtk4::Align::Center);
    validate_btn.set_margin_top(8);
    page.append(&validate_btn);

    let status_label = gtk4::Label::new(None);
    status_label.set_halign(gtk4::Align::Center);
    page.append(&status_label);

    {
        let api_key_row_clone = api_key_row.clone();
        let status_label_clone = status_label.clone();
        validate_btn.connect_clicked(move |_| {
            let key = api_key_row_clone.text().to_string();
            if key.is_empty() {
                status_label_clone.set_text("Please enter an API key.");
                return;
            }
            status_label_clone.set_text("Validating\u{2026}");

            let (tx, rx) =
                std::sync::mpsc::channel::<Result<crate::core::nexus::NexusUser, String>>();

            std::thread::spawn(move || {
                let client = crate::core::nexus::NexusClient::new(&key);
                let _ = tx.send(client.validate());
            });

            let status_label2 = status_label_clone.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                match rx.try_recv() {
                    Ok(Ok(user)) => {
                        status_label2.set_text(&format!(
                            "\u{2714} Logged in as {} ({})",
                            user.username,
                            if user.is_premium { "Premium" } else { "Member" }
                        ));
                        glib::ControlFlow::Break
                    }
                    Ok(Err(e)) => {
                        status_label2.set_text(&format!("\u{2718} Validation failed: {e}"));
                        glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        status_label2.set_text("\u{2718} Validation error");
                        glib::ControlFlow::Break
                    }
                }
            });
        });
    }

    // Spacer
    let spacer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    page.append(&spacer);

    // Bottom buttons
    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk4::Align::End);

    let skip_btn = gtk4::Button::with_label("Skip");
    let finish_btn = gtk4::Button::with_label("Finish");
    finish_btn.add_css_class("suggested-action");

    btn_box.append(&skip_btn);
    btn_box.append(&finish_btn);
    page.append(&btn_box);

    // Skip handler
    {
        let wizard_window_clone = wizard_window.clone();
        let config_clone = Rc::clone(&config);
        let selected_game_clone = Rc::clone(&selected_game);
        let on_finish_clone = Rc::clone(&on_finish);
        skip_btn.connect_clicked(move |_| {
            finish_wizard(
                &wizard_window_clone,
                Rc::clone(&config_clone),
                Rc::clone(&selected_game_clone),
                None,
                Rc::clone(&on_finish_clone),
            );
        });
    }

    // Finish handler
    {
        let wizard_window_clone = wizard_window.clone();
        let config_clone = Rc::clone(&config);
        let selected_game_clone = Rc::clone(&selected_game);
        let api_key_row_clone = api_key_row.clone();
        let on_finish_clone = Rc::clone(&on_finish);
        finish_btn.connect_clicked(move |_| {
            let key = api_key_row_clone.text().to_string();
            let api_key = if key.is_empty() { None } else { Some(key) };
            finish_wizard(
                &wizard_window_clone,
                Rc::clone(&config_clone),
                Rc::clone(&selected_game_clone),
                api_key,
                Rc::clone(&on_finish_clone),
            );
        });
    }

    page
}

fn finish_wizard(
    wizard_window: &adw::Window,
    config: Rc<RefCell<AppConfig>>,
    selected_game: Rc<RefCell<Option<(GameKind, std::path::PathBuf)>>>,
    api_key: Option<String>,
    on_finish: Rc<dyn Fn()>,
) {
    {
        let mut cfg = config.borrow_mut();
        cfg.first_run = false;

        if let Some(key) = api_key {
            cfg.nexus_api_key = Some(key);
        }

        if let Some((kind, path)) = selected_game.borrow().clone() {
            let game = Game::new(kind, path);
            let game_id = game.id.clone();
            cfg.games.push(game);
            cfg.current_game_id = Some(game_id);
        }

        cfg.save();
    }

    wizard_window.destroy();
    on_finish();
}
