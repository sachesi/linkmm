use std::cell::RefCell;
use std::rc::Rc;

use gio;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::games::{GameKind, UmuGameConfig};
use crate::core::steam;

use super::GameSelection;

#[allow(clippy::type_complexity)]
pub(super) fn build_game_select_page(
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

    {
        let updater = Rc::clone(&update_umu_selection);
        exe_row.connect_changed(move |_| updater());
    }
    {
        let updater = Rc::clone(&update_umu_selection);
        prefix_row.connect_changed(move |_| updater());
    }
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

pub(super) fn show_add_game_dialog(
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
