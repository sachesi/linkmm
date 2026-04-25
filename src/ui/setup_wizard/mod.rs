use std::cell::RefCell;
use std::rc::Rc;

use gio;
use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::{Game, GameKind, UmuGameConfig};
use crate::core::steam;
use crate::core::umu;

mod fomod;
pub(crate) use fomod::show_fomod_wizard;
pub use fomod::show_fomod_wizard_from_library;

/// Represents how the user chose to set up a game in the wizard.
#[derive(Clone, Debug)]
enum GameSelection {
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
    let welcome_page = build_welcome_page();
    stack.add_named(&welcome_page, Some("welcome"));

    // --- Page 2: Select Game ---
    let detected_games = steam::detect_games();
    let selected_game: Rc<RefCell<Option<GameSelection>>> = Rc::new(RefCell::new(None));

    let (game_page, selected_game_clone) =
        build_game_select_page(&stack, detected_games, Rc::clone(&selected_game));
    stack.add_named(&game_page, Some("select_game"));

    // --- Page 3: App Directory ---
    let selected_app_dir: Rc<RefCell<Option<std::path::PathBuf>>> = Rc::new(RefCell::new(None));

    let app_dir_page = build_app_dir_page(&stack, Rc::clone(&selected_app_dir));
    stack.add_named(&app_dir_page, Some("app_dir"));

    // --- Page 4: NexusMods API Key ---
    let nexus_page = build_nexus_page(
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

#[allow(clippy::type_complexity)]
fn build_game_select_page(
    stack: &gtk4::Stack,
    detected_games: Vec<steam::DetectedSteamGame>,
    selected_game: Rc<RefCell<Option<GameSelection>>>,
) -> (gtk4::Box, Rc<RefCell<Option<GameSelection>>>) {
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

    // All radio check buttons share a single group so only one can be active
    let check_buttons: Rc<RefCell<Vec<gtk4::CheckButton>>> = Rc::new(RefCell::new(Vec::new()));

    if detected_games.is_empty() {
        let no_games = adw::StatusPage::builder()
            .title("No Steam Games Found")
            .description("No Steam games were detected.\nYou can add a game manually or set up a non-Steam game using UMU below.")
            .icon_name("edit-find-symbolic")
            .build();
        inner_box.append(&no_games);
    } else {
        let section_label = gtk4::Label::new(Some("Auto-Detected (Steam)"));
        section_label.add_css_class("heading");
        section_label.set_halign(gtk4::Align::Start);
        inner_box.append(&section_label);

        let game_list = gtk4::ListBox::new();
        game_list.add_css_class("boxed-list");
        game_list.set_selection_mode(gtk4::SelectionMode::None);

        for detected in &detected_games {
            let kind = &detected.kind;
            let path = &detected.path;
            let row = adw::ActionRow::builder()
                .title(kind.display_name())
                .subtitle(path.to_string_lossy().as_ref())
                .build();

            let check = gtk4::CheckButton::new();
            row.add_prefix(&check);
            row.set_activatable_widget(Some(&check));

            check_buttons.borrow_mut().push(check.clone());

            let kind_clone = detected.kind.clone();
            let app_id = detected.app_id;
            let path_clone = detected.path.clone();
            let selected_game_clone = Rc::clone(&selected_game);
            let all_checks = Rc::clone(&check_buttons);

            check.connect_toggled(move |this_check| {
                if this_check.is_active() {
                    for btn in all_checks.borrow().iter() {
                        if btn != this_check {
                            btn.set_active(false);
                        }
                    }
                    *selected_game_clone.borrow_mut() = Some(GameSelection::Steam {
                        kind: kind_clone.clone(),
                        app_id,
                        path: path_clone.clone(),
                    });
                } else {
                    let mut sel = selected_game_clone.borrow_mut();
                    if let Some(GameSelection::Steam { kind: ref k, .. }) = *sel {
                        if k == &kind_clone {
                            *sel = None;
                        }
                    }
                }
            });

            game_list.append(&row);
        }

        inner_box.append(&game_list);
    }

    // ── Custom (UMU) section ──────────────────────────────────────────────
    let umu_section_label = gtk4::Label::new(Some("Custom (Non-Steam)"));
    umu_section_label.add_css_class("heading");
    umu_section_label.set_halign(gtk4::Align::Start);
    umu_section_label.set_margin_top(12);
    inner_box.append(&umu_section_label);

    let umu_list = gtk4::ListBox::new();
    umu_list.add_css_class("boxed-list");
    umu_list.set_selection_mode(gtk4::SelectionMode::None);

    let umu_row = adw::ActionRow::builder()
        .title("Custom")
        .subtitle("Setup using UMU (Non-Steam)")
        .build();

    let umu_check = gtk4::CheckButton::new();
    umu_row.add_prefix(&umu_check);
    umu_row.set_activatable_widget(Some(&umu_check));

    check_buttons.borrow_mut().push(umu_check.clone());

    // A revealer that shows the UMU configuration fields when selected
    let umu_config_revealer = gtk4::Revealer::new();
    umu_config_revealer.set_transition_type(gtk4::RevealerTransitionType::SlideDown);
    umu_config_revealer.set_reveal_child(false);

    let umu_config_box = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    umu_config_box.set_margin_top(8);

    let umu_prefs = adw::PreferencesGroup::builder()
        .title("UMU Game Configuration")
        .description(
            "Select the game executable — the game is detected automatically from its name. \
             Prefix and Proton are optional: leave them blank to use umu\u{2019}s smart defaults \
             (prefix: ~/.local/share/umu/default \u{2022} Proton: auto-download latest GE-Proton).",
        )
        .build();

    // Executable path
    let exe_row = adw::EntryRow::builder().title("Game Executable").build();
    let browse_exe_btn = gtk4::Button::new();
    browse_exe_btn.set_icon_name("folder-open-symbolic");
    browse_exe_btn.set_valign(gtk4::Align::Center);
    exe_row.add_suffix(&browse_exe_btn);
    umu_prefs.add(&exe_row);

    // Detected game label
    let detected_kind_label = gtk4::Label::new(Some("No executable selected"));
    detected_kind_label.set_halign(gtk4::Align::Start);
    detected_kind_label.add_css_class("dim-label");
    detected_kind_label.set_margin_start(12);

    // Prefix path (optional) — default: ~/.local/share/umu/default
    let prefix_row = adw::EntryRow::builder()
        .title("Wine/Proton Prefix — default: ~/.local/share/umu/default")
        .build();
    let browse_prefix_btn = gtk4::Button::new();
    browse_prefix_btn.set_icon_name("folder-open-symbolic");
    browse_prefix_btn.set_valign(gtk4::Align::Center);
    prefix_row.add_suffix(&browse_prefix_btn);
    umu_prefs.add(&prefix_row);

    // Proton path (optional) — default: auto-download latest GE-Proton
    let proton_row = adw::EntryRow::builder()
        .title("Proton Path — default: auto-download latest GE-Proton")
        .build();
    let browse_proton_btn = gtk4::Button::new();
    browse_proton_btn.set_icon_name("folder-open-symbolic");
    browse_proton_btn.set_valign(gtk4::Align::Center);
    proton_row.add_suffix(&browse_proton_btn);
    umu_prefs.add(&proton_row);

    umu_config_box.append(&umu_prefs);
    umu_config_box.append(&detected_kind_label);
    umu_config_revealer.set_child(Some(&umu_config_box));

    umu_list.append(&umu_row);
    inner_box.append(&umu_list);
    inner_box.append(&umu_config_revealer);

    // Track the detected GameKind from the chosen executable
    let detected_umu_kind: Rc<RefCell<Option<GameKind>>> = Rc::new(RefCell::new(None));

    // Toggle UMU config visibility
    {
        let revealer = umu_config_revealer.clone();
        let all_checks = Rc::clone(&check_buttons);
        let selected_game_clone = Rc::clone(&selected_game);
        umu_check.connect_toggled(move |this_check| {
            revealer.set_reveal_child(this_check.is_active());
            if this_check.is_active() {
                for btn in all_checks.borrow().iter() {
                    if btn != this_check {
                        btn.set_active(false);
                    }
                }
                // Selection will be set when user picks an executable
            } else {
                let mut sel = selected_game_clone.borrow_mut();
                if matches!(*sel, Some(GameSelection::Umu { .. })) {
                    *sel = None;
                }
            }
        });
    }

    // Update selection whenever exe/prefix/proton fields change
    let update_umu_selection = {
        let exe_row_c = exe_row.clone();
        let prefix_row_c = prefix_row.clone();
        let proton_row_c = proton_row.clone();
        let detected_kind_c = Rc::clone(&detected_umu_kind);
        let selected_game_c = Rc::clone(&selected_game);
        let kind_label = detected_kind_label.clone();
        let umu_check_c = umu_check.clone();

        Rc::new(move || {
            if !umu_check_c.is_active() {
                return;
            }
            let exe_text = exe_row_c.text().to_string();
            if exe_text.is_empty() {
                kind_label.set_text("No executable selected");
                kind_label.remove_css_class("success");
                kind_label.remove_css_class("error");
                kind_label.add_css_class("dim-label");
                *detected_kind_c.borrow_mut() = None;
                *selected_game_c.borrow_mut() = None;
                return;
            }
            let exe_path = std::path::PathBuf::from(&exe_text);
            let exe_name = exe_path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            match GameKind::from_executable(exe_name) {
                Some(kind) => {
                    kind_label.set_text(&format!("Detected: {}", kind.display_name()));
                    kind_label.remove_css_class("dim-label");
                    kind_label.remove_css_class("error");
                    kind_label.add_css_class("success");
                    *detected_kind_c.borrow_mut() = Some(kind.clone());

                    // Derive root_path as the parent directory of the executable
                    let root_path = exe_path
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| exe_path.clone());

                    let prefix_text = prefix_row_c.text().to_string();
                    let proton_text = proton_row_c.text().to_string();

                    let umu_cfg = UmuGameConfig {
                        exe_path: exe_path.clone(),
                        prefix_path: if prefix_text.is_empty() {
                            None
                        } else {
                            Some(std::path::PathBuf::from(prefix_text))
                        },
                        proton_path: if proton_text.is_empty() {
                            None
                        } else {
                            Some(std::path::PathBuf::from(proton_text))
                        },
                    };
                    *selected_game_c.borrow_mut() = Some(GameSelection::Umu {
                        kind,
                        root_path,
                        umu_cfg,
                    });
                }
                None => {
                    kind_label.set_text(&format!(
                        "Unsupported executable: \u{201c}{}\u{201d}",
                        exe_name
                    ));
                    kind_label.remove_css_class("dim-label");
                    kind_label.remove_css_class("success");
                    kind_label.add_css_class("error");
                    *detected_kind_c.borrow_mut() = None;
                    *selected_game_c.borrow_mut() = None;
                }
            }
        })
    };

    // Wire up exe_row changed
    {
        let updater = Rc::clone(&update_umu_selection);
        exe_row.connect_changed(move |_| updater());
    }
    // Wire up prefix_row changed
    {
        let updater = Rc::clone(&update_umu_selection);
        prefix_row.connect_changed(move |_| updater());
    }
    // Wire up proton_row changed
    {
        let updater = Rc::clone(&update_umu_selection);
        proton_row.connect_changed(move |_| updater());
    }

    // Browse button for executable
    {
        let exe_row_c = exe_row.clone();
        browse_exe_btn.connect_clicked(move |btn| {
            let file_dialog = gtk4::FileDialog::new();
            file_dialog.set_title("Select Game Executable (.exe)");
            let parent = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            let row_c = exe_row_c.clone();
            file_dialog.open(parent.as_ref(), None::<&gio::Cancellable>, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    row_c.set_text(&path.to_string_lossy());
                }
            });
        });
    }

    // Browse button for prefix
    {
        let prefix_row_c = prefix_row.clone();
        browse_prefix_btn.connect_clicked(move |btn| {
            let file_dialog = gtk4::FileDialog::new();
            file_dialog.set_title("Select Wine/Proton Prefix Folder");
            let parent = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            let row_c = prefix_row_c.clone();
            file_dialog.select_folder(parent.as_ref(), None::<&gio::Cancellable>, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    row_c.set_text(&path.to_string_lossy());
                }
            });
        });
    }

    // Browse button for proton
    {
        let proton_row_c = proton_row.clone();
        browse_proton_btn.connect_clicked(move |btn| {
            let file_dialog = gtk4::FileDialog::new();
            file_dialog.set_title("Select Proton Installation Folder");
            let parent = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            let row_c = proton_row_c.clone();
            file_dialog.select_folder(parent.as_ref(), None::<&gio::Cancellable>, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    row_c.set_text(&path.to_string_lossy());
                }
            });
        });
    }

    scrolled.set_child(Some(&inner_box));
    page.append(&scrolled);

    // "Add Game Manually" button (Steam-style manual add)
    let add_manual_btn = gtk4::Button::with_label("Add Steam Game Manually");
    add_manual_btn.set_halign(gtk4::Align::Center);

    let selected_game_for_manual = Rc::clone(&selected_game);
    let all_checks_for_manual = Rc::clone(&check_buttons);
    add_manual_btn.connect_clicked(move |btn| {
        show_add_game_dialog(
            btn,
            Rc::clone(&selected_game_for_manual),
            Rc::clone(&all_checks_for_manual),
        );
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
        stack_clone.set_visible_child_name("app_dir");
    });

    (page, selected_game)
}

fn show_add_game_dialog(
    parent_widget: &gtk4::Button,
    selected_game: Rc<RefCell<Option<GameSelection>>>,
    all_checks: Rc<RefCell<Vec<gtk4::CheckButton>>>,
) {
    let parent_window = parent_widget
        .root()
        .and_then(|r| r.downcast::<gtk4::Window>().ok());

    let dialog = adw::Window::builder()
        .title("Add Steam Game Manually")
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
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        row_clone.set_text(&path.to_string_lossy());
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
            let Some(app_id) = kind.primary_steam_app_id() else {
                return;
            };
            // Uncheck all radio buttons from the game select page
            for btn in all_checks.borrow().iter() {
                btn.set_active(false);
            }
            *selected_game.borrow_mut() = Some(GameSelection::Steam {
                kind: kind.clone(),
                app_id,
                path: root_path,
            });
        }
        dialog_clone2.destroy();
    });

    dialog.present();
}

// ── App Directory page ────────────────────────────────────────────────────────

fn build_app_dir_page(
    stack: &gtk4::Stack,
    selected_app_dir: Rc<RefCell<Option<std::path::PathBuf>>>,
) -> gtk4::Box {
    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    page.set_vexpand(true);
    page.set_margin_start(24);
    page.set_margin_end(24);
    page.set_margin_top(24);
    page.set_margin_bottom(24);

    let header_label = gtk4::Label::new(Some("App Directory"));
    header_label.add_css_class("title-1");
    header_label.set_halign(gtk4::Align::Start);
    page.append(&header_label);

    let desc_label = gtk4::Label::new(Some(
        "Choose where Linkmm will store downloaded mod archives.\n\
         A \u{201c}downloads\u{201d} sub-folder will be created inside this directory.",
    ));
    desc_label.set_wrap(true);
    desc_label.set_halign(gtk4::Align::Start);
    page.append(&desc_label);

    // Default suggestion: ~/Documents/Linkmm
    let default_dir = dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("Linkmm");

    let path_group = adw::PreferencesGroup::builder().title("Directory").build();

    let dir_row = adw::EntryRow::builder().title("App Directory").build();
    dir_row.set_text(&default_dir.to_string_lossy());

    // Pre-fill the shared state with the default
    *selected_app_dir.borrow_mut() = Some(default_dir.clone());

    let browse_btn = gtk4::Button::new();
    browse_btn.set_icon_name("folder-open-symbolic");
    browse_btn.set_valign(gtk4::Align::Center);
    dir_row.add_suffix(&browse_btn);

    path_group.add(&dir_row);
    page.append(&path_group);

    // Keep shared state in sync when the user types directly
    {
        let app_dir_c = Rc::clone(&selected_app_dir);
        dir_row.connect_changed(move |row| {
            let text = row.text().to_string();
            *app_dir_c.borrow_mut() = if text.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(text))
            };
        });
    }

    // Browse button opens a folder-picker dialog
    {
        let dir_row_c = dir_row.clone();
        let app_dir_c = Rc::clone(&selected_app_dir);
        browse_btn.connect_clicked(move |btn| {
            let file_dialog = gtk4::FileDialog::new();
            file_dialog.set_title("Select App Directory");
            let parent = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            let row_c = dir_row_c.clone();
            let app_dir_cc = Rc::clone(&app_dir_c);
            file_dialog.select_folder(parent.as_ref(), None::<&gio::Cancellable>, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    row_c.set_text(&path.to_string_lossy());
                    *app_dir_cc.borrow_mut() = Some(path);
                }
            });
        });
    }

    // Navigation buttons
    let nav_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    nav_box.set_halign(gtk4::Align::End);
    nav_box.set_margin_top(8);
    nav_box.set_vexpand(true);
    nav_box.set_valign(gtk4::Align::End);

    let next_btn = gtk4::Button::with_label("Next");
    next_btn.add_css_class("suggested-action");

    nav_box.append(&next_btn);
    page.append(&nav_box);

    let stack_clone = stack.clone();
    next_btn.connect_clicked(move |_| {
        stack_clone.set_visible_child_name("nexus_key");
    });

    page
}

fn build_nexus_page(
    wizard_window: &adw::Window,
    config: Rc<RefCell<AppConfig>>,
    selected_game: Rc<RefCell<Option<GameSelection>>>,
    selected_app_dir: Rc<RefCell<Option<std::path::PathBuf>>>,
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

    let existing_key = config.borrow().nexus_api_key.clone();
    let (desc_text, desc_class) = if existing_key.is_some() {
        (
            "Your NexusMods API key is already configured. You can keep it or enter a new one.",
            Some("success"),
        )
    } else {
        (
            "Enter your NexusMods API key to browse and download mods.",
            None,
        )
    };

    let desc_label = gtk4::Label::new(Some(desc_text));
    desc_label.set_wrap(true);
    desc_label.set_halign(gtk4::Align::Start);
    if let Some(css) = desc_class {
        desc_label.add_css_class(css);
    }
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

    // Pre-fill the key if one is already configured so the user doesn't have
    // to re-enter it when adding a second game.
    if let Some(ref key) = existing_key {
        api_key_row.set_text(key);
    }

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
        let selected_app_dir_clone = Rc::clone(&selected_app_dir);
        let on_finish_clone = Rc::clone(&on_finish);
        skip_btn.connect_clicked(move |_| {
            finish_wizard(
                &wizard_window_clone,
                Rc::clone(&config_clone),
                Rc::clone(&selected_game_clone),
                Rc::clone(&selected_app_dir_clone),
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
        let selected_app_dir_clone = Rc::clone(&selected_app_dir);
        let api_key_row_clone = api_key_row.clone();
        let on_finish_clone = Rc::clone(&on_finish);
        finish_btn.connect_clicked(move |_| {
            let key = api_key_row_clone.text().to_string();
            let api_key = if key.is_empty() { None } else { Some(key) };
            finish_wizard(
                &wizard_window_clone,
                Rc::clone(&config_clone),
                Rc::clone(&selected_game_clone),
                Rc::clone(&selected_app_dir_clone),
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
    selected_game: Rc<RefCell<Option<GameSelection>>>,
    selected_app_dir: Rc<RefCell<Option<std::path::PathBuf>>>,
    api_key: Option<String>,
    on_finish: Rc<dyn Fn()>,
) {
    let selection = selected_game.borrow().clone();
    let is_umu = matches!(selection, Some(GameSelection::Umu { .. }));

    // If UMU was selected, always run ensure_umu_available so that:
    //   • the binary is downloaded on first setup, AND
    //   • the version tag is persisted even if the binary already exists.
    // We do this in a background thread with a progress dialog.
    if is_umu {
        let progress_dialog = adw::Window::builder()
            .title("Downloading UMU Launcher")
            .modal(true)
            .transient_for(wizard_window)
            .default_width(400)
            .default_height(150)
            .deletable(false)
            .build();

        let pbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        pbox.set_margin_start(24);
        pbox.set_margin_end(24);
        pbox.set_margin_top(24);
        pbox.set_margin_bottom(24);
        pbox.set_valign(gtk4::Align::Center);

        let plabel = gtk4::Label::new(Some(if umu::is_umu_available() {
            "Checking for umu-launcher updates\u{2026}"
        } else {
            "Downloading umu-launcher\u{2026}"
        }));
        let pbar = gtk4::ProgressBar::new();
        pbar.set_show_text(true);

        pbox.append(&plabel);
        pbox.append(&pbar);
        progress_dialog.set_content(Some(&pbox));
        progress_dialog.present();

        let installed_version = config.borrow().umu_installed_version.clone();
        let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();

        std::thread::spawn(move || {
            let result =
                umu::ensure_umu_available(installed_version.as_deref(), |_downloaded, _total| true);
            let _ = tx.send(result.map(|(tag, _path)| tag));
        });

        let wizard_window_c = wizard_window.clone();
        let config_c = Rc::clone(&config);
        let selected_game_c = Rc::clone(&selected_game);
        let selected_app_dir_c = Rc::clone(&selected_app_dir);
        let on_finish_c = Rc::clone(&on_finish);
        let progress_dialog_c = progress_dialog.clone();
        let pbar_c = pbar.clone();

        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            pbar_c.pulse();
            match rx.try_recv() {
                Ok(Ok(new_tag)) => {
                    progress_dialog_c.destroy();
                    // Persist the installed version tag before closing the wizard.
                    config_c.borrow_mut().umu_installed_version = Some(new_tag);
                    finish_wizard_apply(
                        &wizard_window_c,
                        Rc::clone(&config_c),
                        Rc::clone(&selected_game_c),
                        Rc::clone(&selected_app_dir_c),
                        api_key.clone(),
                        Rc::clone(&on_finish_c),
                    );
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    progress_dialog_c.destroy();
                    log::error!("Failed to download umu-launcher: {e}");
                    // Non-fatal: finish the wizard anyway; the background
                    // update check on next launch will retry.
                    finish_wizard_apply(
                        &wizard_window_c,
                        Rc::clone(&config_c),
                        Rc::clone(&selected_game_c),
                        Rc::clone(&selected_app_dir_c),
                        api_key.clone(),
                        Rc::clone(&on_finish_c),
                    );
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    progress_dialog_c.destroy();
                    finish_wizard_apply(
                        &wizard_window_c,
                        Rc::clone(&config_c),
                        Rc::clone(&selected_game_c),
                        Rc::clone(&selected_app_dir_c),
                        api_key.clone(),
                        Rc::clone(&on_finish_c),
                    );
                    glib::ControlFlow::Break
                }
            }
        });

        return;
    }

    finish_wizard_apply(
        wizard_window,
        config,
        selected_game,
        selected_app_dir,
        api_key,
        on_finish,
    );
}

/// Internal helper that applies the wizard selections to the config and closes the dialog.
fn finish_wizard_apply(
    wizard_window: &adw::Window,
    config: Rc<RefCell<AppConfig>>,
    selected_game: Rc<RefCell<Option<GameSelection>>>,
    selected_app_dir: Rc<RefCell<Option<std::path::PathBuf>>>,
    api_key: Option<String>,
    on_finish: Rc<dyn Fn()>,
) {
    {
        let mut cfg = config.borrow_mut();
        cfg.first_run = false;

        if let Some(key) = api_key {
            cfg.nexus_api_key = Some(key);
        }

        let game_opt: Option<Game> = match selected_game.borrow().clone() {
            Some(GameSelection::Steam { kind, app_id, path }) => {
                Some(Game::new_steam_with_app_id(kind, path, app_id))
            }
            Some(GameSelection::Umu {
                kind,
                root_path,
                umu_cfg,
            }) => Some(Game::new_non_steam_umu(kind, root_path, umu_cfg)),
            None => None,
        };

        if let Some(game) = game_opt {
            let game_id = game.id.clone();
            // Only add the game if it isn't already in the list
            if !cfg.games.iter().any(|g| g.id == game_id) {
                cfg.games.push(game);
            }
            cfg.current_game_id = Some(game_id.clone());

            // Store per-game settings for the newly added game
            let gs = cfg.game_settings_mut(&game_id);
            if let Some(app_dir) = selected_app_dir.borrow().clone() {
                gs.app_data_dir = Some(app_dir);
            }
            // Ensure the game always has at least a default profile
            if gs.profiles.is_empty() {
                gs.profiles = crate::core::config::default_active_profile_id_vec();
            }
        }

        // Resolve mods directories based on per-game app_data_dir for all games
        cfg.apply_mods_base_dirs();

        cfg.save();
    }

    wizard_window.destroy();
    on_finish();
}
