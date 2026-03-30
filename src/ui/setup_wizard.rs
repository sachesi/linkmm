use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use gio;
use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::{Game, GameKind};
use crate::core::installer::{
    DependencyOperator, FlagDependency, FomodConfig,
    FomodFile, FomodGroupType, FomodInstallStep, FomodPlugin, FomodPluginType, InstallStrategy,
    PluginDependencies,
};
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
        stack_clone.set_visible_child_name("app_dir");
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
                    if let Ok(file) = result
                        && let Some(path) = file.path() {
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
            *selected_game.borrow_mut() = Some((kind.clone(), root_path));
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

    let path_group = adw::PreferencesGroup::builder()
        .title("Directory")
        .build();

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
            let parent = btn
                .root()
                .and_then(|r| r.downcast::<gtk4::Window>().ok());
            let row_c = dir_row_c.clone();
            let app_dir_cc = Rc::clone(&app_dir_c);
            file_dialog.select_folder(
                parent.as_ref(),
                None::<&gio::Cancellable>,
                move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path() {
                            row_c.set_text(&path.to_string_lossy());
                            *app_dir_cc.borrow_mut() = Some(path);
                        }
                },
            );
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
    selected_game: Rc<RefCell<Option<(GameKind, std::path::PathBuf)>>>,
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
    selected_game: Rc<RefCell<Option<(GameKind, std::path::PathBuf)>>>,
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

        if let Some((kind, path)) = selected_game.borrow().clone() {
            let game = Game::new(kind, path);
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

// ── FOMOD wizard ──────────────────────────────────────────────────────────────

/// Selected plugin indices by `[step_index][group_index][plugin_indices]`.
type FomodSelections = Vec<Vec<Vec<usize>>>;

fn collect_active_flags(
    fomod: &FomodConfig,
    selections: &FomodSelections,
    up_to_step_inclusive: usize,
) -> HashMap<String, HashSet<String>> {
    let mut flags: HashMap<String, HashSet<String>> = HashMap::new();
    for (si, step) in fomod.steps.iter().enumerate() {
        if si > up_to_step_inclusive {
            break;
        }
        if !step_is_visible_with_flags(step, &flags) {
            continue;
        }
        for (gi, group) in step.groups.iter().enumerate() {
            let Some(selected) = selections.get(si).and_then(|s| s.get(gi)) else {
                continue;
            };
            for &pi in selected {
                let Some(plugin) = group.plugins.get(pi) else {
                    continue;
                };
                for flag in &plugin.condition_flags {
                    flags
                        .entry(flag.name.clone())
                        .or_default()
                        .insert(flag.value.clone());
                }
            }
        }
    }
    flags
}

fn flag_dependency_matches(
    dep: &FlagDependency,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    if let Some(values) = active_flags.get(&dep.flag) {
        values.contains(&dep.value)
    } else {
        false
    }
}

fn dependencies_match(
    dependencies: &PluginDependencies,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    if dependencies.flags.is_empty() {
        return true;
    }
    match dependencies.operator {
        DependencyOperator::And => dependencies
            .flags
            .iter()
            .all(|dep| flag_dependency_matches(dep, active_flags)),
        DependencyOperator::Or => dependencies
            .flags
            .iter()
            .any(|dep| flag_dependency_matches(dep, active_flags)),
    }
}

fn step_is_visible_with_flags(
    step: &FomodInstallStep,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    let Some(visible) = &step.visible else {
        return true;
    };
    dependencies_match(visible, active_flags)
}

fn plugin_is_visible(
    plugin: &FomodPlugin,
    active_flags: &HashMap<String, HashSet<String>>,
) -> bool {
    let Some(deps) = &plugin.dependencies else {
        return true;
    };
    dependencies_match(deps, active_flags)
}

fn sanitize_step_selection(
    fomod: &FomodConfig,
    selections: &mut FomodSelections,
    step_index: usize,
) {
    let Some(step) = fomod.steps.get(step_index) else {
        return;
    };
    let active_flags = collect_active_flags(fomod, selections, step_index);
    for (gi, group) in step.groups.iter().enumerate() {
        let Some(selected) = selections.get_mut(step_index).and_then(|s| s.get_mut(gi)) else {
            continue;
        };
        let visible: Vec<usize> = group
            .plugins
            .iter()
            .enumerate()
            .filter_map(|(pi, plugin)| {
                if plugin_is_visible(plugin, &active_flags) {
                    Some(pi)
                } else {
                    None
                }
            })
            .collect();
        selected.retain(|pi| visible.contains(pi));
        match group.group_type {
            FomodGroupType::SelectAll => {
                *selected = visible;
            }
            FomodGroupType::SelectExactlyOne => {
                if selected.len() > 1 {
                    selected.truncate(1);
                }
                if selected.is_empty()
                    && let Some(first) = visible.first() {
                        selected.push(*first);
                    }
            }
            FomodGroupType::SelectAtLeastOne => {
                if selected.is_empty()
                    && let Some(first) = visible.first() {
                        selected.push(*first);
                    }
            }
            FomodGroupType::SelectAtMostOne => {
                if selected.len() > 1 {
                    selected.truncate(1);
                }
            }
            FomodGroupType::SelectAny => {}
        }
        selected.sort();
        selected.dedup();
    }
}

fn resolve_fomod_files(fomod: &FomodConfig, selections: &FomodSelections) -> Vec<FomodFile> {
    let mut files: Vec<FomodFile> = fomod.required_files.clone();
    let mut normalized = selections.clone();
    for si in 0..fomod.steps.len() {
        sanitize_step_selection(fomod, &mut normalized, si);
    }
    for (si, step) in fomod.steps.iter().enumerate() {
        let step_flags = if si == 0 {
            HashMap::new()
        } else {
            collect_active_flags(fomod, &normalized, si - 1)
        };
        if !step_is_visible_with_flags(step, &step_flags) {
            continue;
        }
        for (gi, group) in step.groups.iter().enumerate() {
            if let Some(selected) = normalized.get(si).and_then(|s| s.get(gi)) {
                for &pi in selected {
                    if let Some(plugin) = group.plugins.get(pi) {
                        files.extend(plugin.files.iter().cloned());
                    }
                }
            }
        }
    }
    let active_flags = if fomod.steps.is_empty() {
        HashMap::new()
    } else {
        collect_active_flags(fomod, &normalized, fomod.steps.len() - 1)
    };
    for conditional in &fomod.conditional_file_installs {
        if dependencies_match(&conditional.dependencies, &active_flags) {
            files.extend(conditional.files.iter().cloned());
        }
    }
    files
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn show_fomod_wizard(
    parent: Option<&gtk4::Window>,
    archive_name: &str,
    fomod: &FomodConfig,
    images_data: HashMap<String, Vec<u8>>,
    on_install_fn: impl Fn(InstallStrategy) + 'static,
) {
    let mod_display_name = fomod
        .mod_name
        .clone()
        .unwrap_or_else(|| archive_name.to_string());

    // No interactive steps — install required files immediately.
    if fomod.steps.is_empty() {
        let strategy = InstallStrategy::Fomod(fomod.required_files.clone());
        on_install_fn(strategy);
        return;
    }

    // Initialise per-step, per-group selections with defaults.
    let selections: Rc<RefCell<FomodSelections>> = Rc::new(RefCell::new(Vec::new()));
    {
        let mut sel = selections.borrow_mut();
        for step in &fomod.steps {
            let mut step_sel = Vec::new();
            for group in &step.groups {
                let mut gs: Vec<usize> = Vec::new();
                for (idx, plugin) in group.plugins.iter().enumerate() {
                    if matches!(
                        plugin.type_descriptor,
                        FomodPluginType::Required | FomodPluginType::Recommended
                    ) || group.group_type == FomodGroupType::SelectAll
                    {
                        gs.push(idx);
                    }
                }
                if gs.is_empty()
                    && matches!(
                        group.group_type,
                        FomodGroupType::SelectExactlyOne | FomodGroupType::SelectAtLeastOne
                    )
                    && !group.plugins.is_empty()
                {
                    gs.push(0);
                }
                gs.sort();
                gs.dedup();
                step_sel.push(gs);
            }
            sel.push(step_sel);
        }
    }

    // Convert raw image bytes into GPU textures once, up-front.
    let image_cache: Rc<RefCell<HashMap<String, gtk4::gdk::Texture>>> =
        Rc::new(RefCell::new(HashMap::new()));
    {
        let mut cache = image_cache.borrow_mut();
        for (path, bytes) in images_data {
            if let Ok(texture) =
                gtk4::gdk::Texture::from_bytes(&gtk4::glib::Bytes::from_owned(bytes))
            {
                cache.insert(path, texture);
            }
        }
    }

    // ── adw::Dialog ──────────────────────────────────────────────────────────
    let dialog = adw::Dialog::builder()
        .title(format!("Install: {mod_display_name}"))
        .content_width(700)
        .content_height(560)
        .build();

    let nav_view = adw::NavigationView::new();
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&nav_view));
    dialog.set_child(Some(&toast_overlay));

    // ── Shared install callback ───────────────────────────────────────────────
    let dlg = dialog.clone();
    let fomod_rc = Rc::new(fomod.clone());
    let sc_install = Rc::clone(&selections);
    let fc_install = Rc::clone(&fomod_rc);
    let on_install_fn = Rc::new(on_install_fn);
    let on_install: Rc<dyn Fn()> = Rc::new(move || {
        let files = {
            let sel = sc_install.borrow();
            resolve_fomod_files(&fc_install, &sel)
        };
        dlg.close();
        on_install_fn(InstallStrategy::Fomod(files));
    });

    // Build and push the first visible step page.
    let initial_flags = HashMap::new();
    if let Some(first_idx) = (0..fomod_rc.steps.len())
        .find(|&i| step_is_visible_with_flags(&fomod_rc.steps[i], &initial_flags))
    {
        let page = build_fomod_nav_page(
            first_idx,
            Rc::clone(&fomod_rc),
            Rc::clone(&selections),
            Rc::clone(&image_cache),
            nav_view.clone(),
            Rc::clone(&on_install),
        );
        nav_view.push(&page);
    }

    dialog.present(parent);
}

/// Build one `adw::NavigationPage` for `step_idx` in `fomod`, following the
/// GNOME HIG:
///
/// * `adw::ToolbarView` with `adw::HeaderBar` (provides the back button).
/// * Plugin groups rendered as `adw::PreferencesGroup` + `gtk4::ListBox` with
///   `boxed-list` class and `adw::ActionRow` per plugin.
/// * Small image thumbnails (96×72) as row prefixes.
/// * Next / Install pill button in the `ToolbarView` bottom bar.
/// * Next is insensitive until every required group has a valid selection.
fn build_fomod_nav_page(
    step_idx: usize,
    fomod: Rc<FomodConfig>,
    selections: Rc<RefCell<FomodSelections>>,
    image_cache: Rc<RefCell<HashMap<String, gtk4::gdk::Texture>>>,
    nav_view: adw::NavigationView,
    on_install: Rc<dyn Fn()>,
) -> adw::NavigationPage {
    // Ensure selections for this step are consistent with its constraints.
    {
        let mut sel = selections.borrow_mut();
        sanitize_step_selection(&fomod, &mut sel, step_idx);
    }

    let step = &fomod.steps[step_idx];

    // Flags set by all steps that precede this one (used for plugin-level
    // visibility within the current step).
    let prev_flags = {
        let sel = selections.borrow();
        if step_idx > 0 {
            collect_active_flags(&fomod, &sel, step_idx - 1)
        } else {
            HashMap::new()
        }
    };

    // ── NavigationPage ───────────────────────────────────────────────────────
    let page = adw::NavigationPage::builder()
        .title(&step.name)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&adw::HeaderBar::new());

    // ── Forward button (Next / Install) ──────────────────────────────────────
    let is_last_initially = {
        let sel = selections.borrow();
        let flags = collect_active_flags(&fomod, &sel, step_idx);
        (step_idx + 1..fomod.steps.len())
            .find(|&i| step_is_visible_with_flags(&fomod.steps[i], &flags))
            .is_none()
    };

    let fwd_btn = gtk4::Button::with_label(if is_last_initially {
        "Install"
    } else {
        "Next"
    });
    fwd_btn.add_css_class("suggested-action");
    fwd_btn.add_css_class("pill");
    fwd_btn.set_halign(gtk4::Align::Center);
    fwd_btn.set_margin_top(12);
    fwd_btn.set_margin_bottom(12);

    fwd_btn.set_sensitive(step_selection_is_valid(
        &fomod,
        step_idx,
        &selections.borrow(),
    ));

    // ── Forward-navigation handler ───────────────────────────────────────────
    {
        let fc = Rc::clone(&fomod);
        let sc = Rc::clone(&selections);
        let ic = Rc::clone(&image_cache);
        let nv = nav_view.clone();
        let oi = Rc::clone(&on_install);
        fwd_btn.connect_clicked(move |_| {
            let next_flags = {
                let sel = sc.borrow();
                collect_active_flags(&fc, &sel, step_idx)
            };
            let next_idx = (step_idx + 1..fc.steps.len())
                .find(|&i| step_is_visible_with_flags(&fc.steps[i], &next_flags));
            if let Some(next_idx) = next_idx {
                let next_page = build_fomod_nav_page(
                    next_idx,
                    Rc::clone(&fc),
                    Rc::clone(&sc),
                    Rc::clone(&ic),
                    nv.clone(),
                    Rc::clone(&oi),
                );
                nv.push(&next_page);
            } else {
                oi();
            }
        });
    }

    // ── Empty step guard (§4f) ────────────────────────────────────────────────
    let all_groups_empty = step.groups.iter().all(|g| {
        g.plugins.iter().all(|p| !plugin_is_visible(p, &prev_flags))
    });

    if all_groups_empty {
        let empty = adw::StatusPage::builder()
            .icon_name("dialog-information-symbolic")
            .title("No Options Available")
            .description("All options in this step are hidden based on your earlier selections.")
            .vexpand(true)
            .build();
        toolbar_view.set_content(Some(&empty));
        fwd_btn.set_sensitive(true);
        let action_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        action_bar.set_halign(gtk4::Align::Center);
        action_bar.append(&fwd_btn);
        toolbar_view.add_bottom_bar(&action_bar);
        page.set_child(Some(&toolbar_view));
        return page;
    }

    // ── Content box (Clamp + ScrolledWindow) ─────────────────────────────────
    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 18);
    content_box.set_margin_top(18);
    content_box.set_margin_bottom(18);
    content_box.set_margin_start(18);
    content_box.set_margin_end(18);

    // ── Validation banner (§4e) ───────────────────────────────────────────────
    let validation_banner = adw::Banner::builder()
        .title("Please make a selection in all required groups to continue")
        .revealed(!step_selection_is_valid(&fomod, step_idx, &selections.borrow()))
        .build();
    content_box.append(&validation_banner);

    let fwd_btn_ref = fwd_btn.clone();
    let validation_banner_ref = validation_banner.clone();

    // ── Plugin groups ────────────────────────────────────────────────────────
    for (gi, group) in step.groups.iter().enumerate() {
        // Only include plugins that pass dependency-flag visibility check.
        let visible_plugins: Vec<usize> = group
            .plugins
            .iter()
            .enumerate()
            .filter_map(|(pi, plugin)| {
                if plugin_is_visible(plugin, &prev_flags) {
                    Some(pi)
                } else {
                    None
                }
            })
            .collect();

        if visible_plugins.is_empty() {
            continue;
        }

        // §4a: group description instead of title suffix.
        let hint = match group.group_type {
            FomodGroupType::SelectExactlyOne => "Select one option",
            FomodGroupType::SelectAtMostOne => "Select at most one option",
            FomodGroupType::SelectAtLeastOne => "Select at least one option",
            FomodGroupType::SelectAll => "All options are included",
            FomodGroupType::SelectAny => "Select any number of options",
        };

        let pref_group = adw::PreferencesGroup::builder()
            .title(&group.name)
            .description(hint)
            .build();

        let list = gtk4::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk4::SelectionMode::None);
        list.set_hexpand(true);

        let use_radio = matches!(
            group.group_type,
            FomodGroupType::SelectExactlyOne | FomodGroupType::SelectAtMostOne
        );
        let mut radio_leader: Option<gtk4::CheckButton> = None;

        for &pi in &visible_plugins {
            let plugin = &group.plugins[pi];
            let row = adw::ActionRow::builder().title(&plugin.name).build();

            // §4c: CRLF cleanup + 3-line truncation in subtitle.
            // `.lines()` handles \r\n, \r, and \n uniformly.
            if let Some(ref d) = plugin.description {
                let truncated: String = d.lines().take(3).collect::<Vec<_>>().join("\n");
                let trimmed = truncated.trim();
                if !trimmed.is_empty() {
                    row.set_subtitle(trimmed);
                }
            }

            // §4b: Small thumbnail prefix (96×72, cover crop).
            if let Some(ref ip) = plugin.image_path {
                if let Some(texture) = image_cache.borrow().get(ip).cloned() {
                    let thumb = gtk4::Picture::new();
                    thumb.set_size_request(96, 72);
                    thumb.set_paintable(Some(&texture));
                    thumb.set_content_fit(gtk4::ContentFit::Cover);
                    thumb.set_valign(gtk4::Align::Center);
                    let thumb_frame = gtk4::Frame::new(None);
                    thumb_frame.set_valign(gtk4::Align::Center);
                    thumb_frame.set_child(Some(&thumb));
                    row.add_prefix(&thumb_frame);
                }
            }

            match group.group_type {
                FomodGroupType::SelectAll => {
                    // All plugins are auto-selected; show a read-only badge
                    // instead of an interactive checkbox.
                    row.set_sensitive(false);
                    row.set_activatable(false);
                    let badge = gtk4::Label::new(Some("Included"));
                    badge.add_css_class("dim-label");
                    badge.add_css_class("caption");
                    badge.set_valign(gtk4::Align::Center);
                    row.add_suffix(&badge);
                }
                _ => {
                    let check = gtk4::CheckButton::new();
                    check.set_valign(gtk4::Align::Center);

                    // Use GTK radio-group semantics — no manual uncheck loops.
                    if use_radio {
                        if let Some(ref leader) = radio_leader {
                            check.set_group(Some(leader));
                        } else {
                            radio_leader = Some(check.clone());
                        }
                    }

                    // Restore persisted selection state.
                    {
                        let sel = selections.borrow();
                        if let Some(gs) = sel.get(step_idx).and_then(|s| s.get(gi)) {
                            check.set_active(gs.contains(&pi));
                        }
                    }

                    // Required plugins: pre-checked and locked.
                    if plugin.type_descriptor == FomodPluginType::Required {
                        check.set_active(true);
                        check.set_sensitive(false);
                    }

                    // NotUsable plugins: greyed out with an explanatory tooltip.
                    if plugin.type_descriptor == FomodPluginType::NotUsable {
                        row.set_sensitive(false);
                        row.set_tooltip_text(Some("Not available for your current setup"));
                    }

                    // Update selections and button/banner state on every toggle.
                    let sel_c = Rc::clone(&selections);
                    let fomod_c = Rc::clone(&fomod);
                    let nb = fwd_btn_ref.clone();
                    let vb = validation_banner_ref.clone();
                    let is_radio = use_radio;
                    check.connect_toggled(move |btn| {
                        let mut sel = sel_c.borrow_mut();
                        if let Some(gs) =
                            sel.get_mut(step_idx).and_then(|s| s.get_mut(gi))
                        {
                            if btn.is_active() {
                                if is_radio {
                                    gs.clear();
                                }
                                if !gs.contains(&pi) {
                                    gs.push(pi);
                                }
                            } else {
                                gs.retain(|&x| x != pi);
                            }
                        }
                        // Sensitivity: re-validate required groups.
                        let valid = step_selection_is_valid(&fomod_c, step_idx, &sel);
                        nb.set_sensitive(valid);
                        vb.set_revealed(!valid);
                        // Label: re-evaluate step visibility so Install/Next
                        // stays accurate when selections affect later steps.
                        let cur_flags = collect_active_flags(&fomod_c, &sel, step_idx);
                        let still_last = (step_idx + 1..fomod_c.steps.len())
                            .find(|&i| {
                                step_is_visible_with_flags(&fomod_c.steps[i], &cur_flags)
                            })
                            .is_none();
                        nb.set_label(if still_last { "Install" } else { "Next" });
                    });

                    // Type badge (Required / Recommended / Not Usable).
                    let tl = match plugin.type_descriptor {
                        FomodPluginType::Required => Some("Required"),
                        FomodPluginType::Recommended => Some("Recommended"),
                        FomodPluginType::NotUsable => Some("Not Usable"),
                        FomodPluginType::Optional => None,
                    };
                    if let Some(lt) = tl {
                        let badge = gtk4::Label::new(Some(lt));
                        badge.add_css_class("dim-label");
                        badge.add_css_class("caption");
                        badge.set_valign(gtk4::Align::Center);
                        row.add_suffix(&badge);
                    }

                    row.add_suffix(&check);
                    row.set_activatable_widget(Some(&check));
                }
            }

            list.append(&row);
        }

        pref_group.add(&list);
        content_box.append(&pref_group);
    }

    // ── Scroll + Clamp (max 700 px, GNOME HIG standard) ─────────────────────
    let scroll = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .build();
    let clamp = adw::Clamp::builder()
        .maximum_size(700)
        .child(&content_box)
        .build();
    scroll.set_child(Some(&clamp));
    toolbar_view.set_content(Some(&scroll));

    // ── Bottom action bar — Next / Install pill button ────────────────────────
    let action_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    action_bar.set_halign(gtk4::Align::Center);
    action_bar.append(&fwd_btn);
    toolbar_view.add_bottom_bar(&action_bar);

    page.set_child(Some(&toolbar_view));

    page
}

/// Returns `true` when every group in `step_idx` that mandates a selection
/// (SelectExactlyOne, SelectAtLeastOne) has at least one plugin chosen.
fn step_selection_is_valid(
    fomod: &FomodConfig,
    step_idx: usize,
    selections: &FomodSelections,
) -> bool {
    let Some(step) = fomod.steps.get(step_idx) else {
        return true;
    };
    for (gi, group) in step.groups.iter().enumerate() {
        let count = selections
            .get(step_idx)
            .and_then(|s| s.get(gi))
            .map(|gs| gs.len())
            .unwrap_or(0);
        match group.group_type {
            FomodGroupType::SelectExactlyOne | FomodGroupType::SelectAtLeastOne => {
                if count == 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
pub fn show_fomod_wizard_from_library(
    parent: Option<&gtk4::Window>,
    archive_path: &Path,
    archive_name: &str,
    game: &Game,
    config: &Rc<RefCell<AppConfig>>,
    container: &gtk4::Box,
    fomod: &FomodConfig,
    game_rc: &Rc<Option<Game>>,
) {
    let ap = archive_path.to_path_buf();
    let an = archive_name.to_string();
    let gc = game.clone();
    let cc = Rc::clone(config);
    let cont = container.clone();
    let grc = Rc::clone(game_rc);
    show_fomod_wizard(
        parent,
        archive_name,
        fomod,
        HashMap::new(),
        move |strategy| {
            crate::ui::downloads::do_install(
                &ap, &an, &gc, &cc, &cont, false, "", &strategy, &grc, None,
            );
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::installer::{ConditionFlag, ConditionalFileInstall, FomodPluginGroup};

    fn test_plugin(
        name: &str,
        file_source: &str,
        condition_flags: Vec<ConditionFlag>,
        dependencies: Option<PluginDependencies>,
    ) -> FomodPlugin {
        FomodPlugin {
            name: name.to_string(),
            description: None,
            image_path: None,
            files: vec![FomodFile {
                source: file_source.to_string(),
                destination: "Data".to_string(),
                priority: 0,
            }],
            type_descriptor: FomodPluginType::Optional,
            condition_flags,
            dependencies,
        }
    }

    #[test]
    fn resolve_fomod_files_filters_plus_minus_variants_by_dependency_flags() {
        let mut config = FomodConfig {
            mod_name: Some("Test".to_string()),
            required_files: Vec::new(),
            steps: Vec::new(),
            conditional_file_installs: Vec::new(),
        };
        config.steps.push(FomodInstallStep {
            name: "Flags".to_string(),
            visible: None,
            groups: vec![FomodPluginGroup {
                name: "Feature".to_string(),
                group_type: FomodGroupType::SelectExactlyOne,
                plugins: vec![
                    test_plugin(
                        "Enable +",
                        "flags-plus.txt",
                        vec![ConditionFlag {
                            name: "FeaturePack".to_string(),
                            value: "+".to_string(),
                        }],
                        None,
                    ),
                    test_plugin(
                        "Enable -",
                        "flags-minus.txt",
                        vec![ConditionFlag {
                            name: "FeaturePack".to_string(),
                            value: "-".to_string(),
                        }],
                        None,
                    ),
                ],
            }],
        });
        config.steps.push(FomodInstallStep {
            name: "Variant".to_string(),
            visible: None,
            groups: vec![FomodPluginGroup {
                name: "Pick".to_string(),
                group_type: FomodGroupType::SelectAny,
                plugins: vec![
                    test_plugin(
                        "Plus variant",
                        "plus-variant.txt",
                        Vec::new(),
                        Some(PluginDependencies {
                            operator: DependencyOperator::And,
                            flags: vec![FlagDependency {
                                flag: "FeaturePack".to_string(),
                                value: "+".to_string(),
                            }],
                        }),
                    ),
                    test_plugin(
                        "Minus variant",
                        "minus-variant.txt",
                        Vec::new(),
                        Some(PluginDependencies {
                            operator: DependencyOperator::And,
                            flags: vec![FlagDependency {
                                flag: "FeaturePack".to_string(),
                                value: "-".to_string(),
                            }],
                        }),
                    ),
                ],
            }],
        });

        // Simulate stale UI state selecting both variants in step 2 while step 1
        // has "+" selected.
        let selections = vec![vec![vec![0]], vec![vec![0, 1]]];
        let files = resolve_fomod_files(&config, &selections);
        let sources: Vec<String> = files.into_iter().map(|f| f.source).collect();
        assert!(sources.contains(&"plus-variant.txt".to_string()));
        assert!(!sources.contains(&"minus-variant.txt".to_string()));
    }

    #[test]
    fn sanitize_step_selection_respects_at_most_and_exactly_one_rules() {
        let config = FomodConfig {
            mod_name: Some("Test".to_string()),
            required_files: Vec::new(),
            conditional_file_installs: Vec::new(),
            steps: vec![FomodInstallStep {
                name: "Main".to_string(),
                visible: None,
                groups: vec![
                    FomodPluginGroup {
                        name: "Exactly one".to_string(),
                        group_type: FomodGroupType::SelectExactlyOne,
                        plugins: vec![
                            test_plugin("A", "a.txt", Vec::new(), None),
                            test_plugin("B", "b.txt", Vec::new(), None),
                        ],
                    },
                    FomodPluginGroup {
                        name: "At most one".to_string(),
                        group_type: FomodGroupType::SelectAtMostOne,
                        plugins: vec![
                            test_plugin("C", "c.txt", Vec::new(), None),
                            test_plugin("D", "d.txt", Vec::new(), None),
                        ],
                    },
                ],
            }],
        };
        let mut selections = vec![vec![vec![0, 1], vec![0, 1]]];
        sanitize_step_selection(&config, &mut selections, 0);
        assert_eq!(selections[0][0], vec![0]);
        assert_eq!(selections[0][1], vec![0]);

        let mut empty_at_most_one = vec![vec![vec![1], vec![]]];
        sanitize_step_selection(&config, &mut empty_at_most_one, 0);
        assert_eq!(empty_at_most_one[0][1], Vec::<usize>::new());
    }

    #[test]
    fn resolve_fomod_files_applies_step_visibility_and_conditional_files() {
        let config = FomodConfig {
            mod_name: Some("Test".to_string()),
            required_files: Vec::new(),
            steps: vec![
                FomodInstallStep {
                    name: "Flags".to_string(),
                    visible: None,
                    groups: vec![FomodPluginGroup {
                        name: "Feature".to_string(),
                        group_type: FomodGroupType::SelectExactlyOne,
                        plugins: vec![
                            test_plugin(
                                "Enable Underwear",
                                "underwear-on.txt",
                                vec![ConditionFlag {
                                    name: "bUnderwear".to_string(),
                                    value: "On".to_string(),
                                }],
                                None,
                            ),
                            test_plugin("Disable Underwear", "underwear-off.txt", Vec::new(), None),
                        ],
                    }],
                },
                FomodInstallStep {
                    name: "Underwear Options".to_string(),
                    visible: Some(PluginDependencies {
                        operator: DependencyOperator::And,
                        flags: vec![FlagDependency {
                            flag: "bUnderwear".to_string(),
                            value: "On".to_string(),
                        }],
                    }),
                    groups: vec![FomodPluginGroup {
                        name: "Color".to_string(),
                        group_type: FomodGroupType::SelectExactlyOne,
                        plugins: vec![test_plugin(
                            "Black",
                            "underwear-black.txt",
                            Vec::new(),
                            None,
                        )],
                    }],
                },
            ],
            conditional_file_installs: vec![ConditionalFileInstall {
                dependencies: PluginDependencies {
                    operator: DependencyOperator::And,
                    flags: vec![FlagDependency {
                        flag: "bUnderwear".to_string(),
                        value: "On".to_string(),
                    }],
                },
                files: vec![FomodFile {
                    source: "conditional-underwear.txt".to_string(),
                    destination: "Data".to_string(),
                    priority: 0,
                }],
            }],
        };

        // Step 1 picks "Disable Underwear". Step 2 has stale selection but should
        // not contribute because the step is hidden.
        let hidden_step_selections = vec![vec![vec![1]], vec![vec![0]]];
        let hidden_files = resolve_fomod_files(&config, &hidden_step_selections);
        let hidden_sources: Vec<String> = hidden_files.into_iter().map(|f| f.source).collect();
        assert!(!hidden_sources.contains(&"underwear-black.txt".to_string()));
        assert!(!hidden_sources.contains(&"conditional-underwear.txt".to_string()));

        // Step 1 picks "Enable Underwear", so step 2 and conditional files apply.
        let visible_step_selections = vec![vec![vec![0]], vec![vec![0]]];
        let visible_files = resolve_fomod_files(&config, &visible_step_selections);
        let visible_sources: Vec<String> = visible_files.into_iter().map(|f| f.source).collect();
        assert!(visible_sources.contains(&"underwear-black.txt".to_string()));
        assert!(visible_sources.contains(&"conditional-underwear.txt".to_string()));
    }

    #[test]
    fn resolve_fomod_files_diamond_skin_pattern_no_direct_plugin_files() {
        // Simulates Diamond Skin: plugins have ONLY conditionFlags (no direct
        // <files> elements).  All actual files come from conditionalFileInstalls.
        let config = FomodConfig {
            mod_name: Some("Diamond Skin".to_string()),
            required_files: Vec::new(),
            steps: vec![FomodInstallStep {
                name: "Body Type".to_string(),
                visible: None,
                groups: vec![FomodPluginGroup {
                    name: "Body".to_string(),
                    group_type: FomodGroupType::SelectExactlyOne,
                    plugins: vec![
                        FomodPlugin {
                            name: "CBBE".to_string(),
                            description: None,
                            image_path: None,
                            files: Vec::new(), // No direct files
                            type_descriptor: FomodPluginType::Optional,
                            condition_flags: vec![ConditionFlag {
                                name: "isCBBE".to_string(),
                                value: "selected".to_string(),
                            }],
                            dependencies: None,
                        },
                        FomodPlugin {
                            name: "UNP".to_string(),
                            description: None,
                            image_path: None,
                            files: Vec::new(), // No direct files
                            type_descriptor: FomodPluginType::Optional,
                            condition_flags: vec![ConditionFlag {
                                name: "isUNP".to_string(),
                                value: "selected".to_string(),
                            }],
                            dependencies: None,
                        },
                    ],
                }],
            }],
            conditional_file_installs: vec![
                ConditionalFileInstall {
                    dependencies: PluginDependencies {
                        operator: DependencyOperator::And,
                        flags: vec![FlagDependency {
                            flag: "isCBBE".to_string(),
                            value: "selected".to_string(),
                        }],
                    },
                    files: vec![FomodFile {
                        source: "CBBE 4K".to_string(),
                        destination: String::new(),
                        priority: 0,
                    }],
                },
                ConditionalFileInstall {
                    dependencies: PluginDependencies {
                        operator: DependencyOperator::And,
                        flags: vec![FlagDependency {
                            flag: "isUNP".to_string(),
                            value: "selected".to_string(),
                        }],
                    },
                    files: vec![FomodFile {
                        source: "UNP 4K".to_string(),
                        destination: String::new(),
                        priority: 0,
                    }],
                },
            ],
        };

        // Select CBBE (index 0): should get CBBE 4K files, not UNP 4K.
        let cbbe_selected = vec![vec![vec![0usize]]];
        let cbbe_files = resolve_fomod_files(&config, &cbbe_selected);
        let cbbe_sources: Vec<String> = cbbe_files.into_iter().map(|f| f.source).collect();
        assert!(
            cbbe_sources.contains(&"CBBE 4K".to_string()),
            "CBBE 4K should be installed when CBBE is selected"
        );
        assert!(
            !cbbe_sources.contains(&"UNP 4K".to_string()),
            "UNP 4K should NOT be installed when CBBE is selected"
        );

        // Select UNP (index 1): should get UNP 4K files, not CBBE 4K.
        let unp_selected = vec![vec![vec![1usize]]];
        let unp_files = resolve_fomod_files(&config, &unp_selected);
        let unp_sources: Vec<String> = unp_files.into_iter().map(|f| f.source).collect();
        assert!(
            unp_sources.contains(&"UNP 4K".to_string()),
            "UNP 4K should be installed when UNP is selected"
        );
        assert!(
            !unp_sources.contains(&"CBBE 4K".to_string()),
            "CBBE 4K should NOT be installed when UNP is selected"
        );
    }
}
