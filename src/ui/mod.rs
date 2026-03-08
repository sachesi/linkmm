use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::Game;
use crate::core::mods::ModDatabase;

pub mod downloads;
pub mod library;
pub mod load_order;
pub mod mod_list;
pub mod settings;
pub mod setup_wizard;

pub fn build_ui(app: &libadwaita::Application) {
    let config = Rc::new(RefCell::new(AppConfig::load_or_default()));
    let (window, on_setup_done) = build_main_window(app, Rc::clone(&config));
    window.present();
    if config.borrow().first_run || config.borrow().games.is_empty() {
        setup_wizard::show_setup_wizard(&window, Rc::clone(&config), on_setup_done);
    }
}

// ── Navigation constants ───────────────────────────────────────────────────

const NAV_LIBRARY: i32 = 0;
const NAV_LOAD_ORDER: i32 = 1;
const NAV_DOWNLOADS: i32 = 2;
const NAV_PREFERENCES: i32 = 3;

// ── Main window ────────────────────────────────────────────────────────────

fn build_main_window(
    app: &libadwaita::Application,
    config: Rc<RefCell<AppConfig>>,
) -> (adw::ApplicationWindow, impl Fn() + 'static) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Linkmm")
        .default_width(1100)
        .default_height(720)
        .build();

    // Register app-level About action
    register_about_action(app, &window);

    let split_view = adw::NavigationSplitView::new();
    split_view.set_min_sidebar_width(200.0);
    split_view.set_max_sidebar_width(320.0);
    split_view.set_sidebar_width_fraction(0.24);
    split_view.set_collapsed(false);

    // ── Sidebar ───────────────────────────────────────────────────────────

    let sidebar_toolbar = adw::ToolbarView::new();
    let sidebar_header = adw::HeaderBar::new();
    // Keep window controls only on the content side
    sidebar_header.set_show_end_title_buttons(false);

    let menu_btn = gtk4::MenuButton::new();
    menu_btn.set_icon_name("open-menu-symbolic");
    let menu = gio::Menu::new();
    menu.append(Some("About Linkmm"), Some("app.about"));
    menu_btn.set_menu_model(Some(&menu));
    sidebar_header.pack_end(&menu_btn);

    sidebar_toolbar.add_top_bar(&sidebar_header);

    // Scrollable sidebar content
    let sidebar_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

    // ── Active Game section ───────────────────────────────────────────────
    sidebar_box.append(&make_section_label("Active Game"));

    let active_game_list = gtk4::ListBox::new();
    active_game_list.add_css_class("boxed-list");
    active_game_list.set_margin_start(12);
    active_game_list.set_margin_end(12);
    active_game_list.set_margin_bottom(12);
    active_game_list.set_selection_mode(gtk4::SelectionMode::None);

    let active_game_row = adw::ActionRow::builder().activatable(true).build();
    let game_icon = gtk4::Image::from_icon_name("applications-games-symbolic");
    active_game_row.add_prefix(&game_icon);

    {
        let cfg = config.borrow();
        update_active_game_row(
            &active_game_row,
            cfg.current_game().map(|g| g.name.as_str()),
        );
    }

    active_game_list.append(&active_game_row);
    sidebar_box.append(&active_game_list);

    // ── Launch Game button ────────────────────────────────────────────────
    //
    // Shows a plain Button when exactly one executable is discovered in the
    // game directory, or an adw::SplitButton (with a dropdown listing all
    // discovered executables) when more than one exist.  Both are placed
    // inside a shared container box; the inactive widget is hidden so the
    // layout stays stable.

    let launch_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    launch_box.set_margin_start(12);
    launch_box.set_margin_end(12);
    launch_box.set_margin_bottom(8);
    launch_box.set_hexpand(true);

    // Plain button (used when there is exactly one known executable)
    let launch_simple_btn = gtk4::Button::builder()
        .label("Launch Game")
        .hexpand(true)
        .visible(false)
        .build();
    launch_simple_btn.add_css_class("suggested-action");

    // Split button (used when multiple executables are discovered)
    let launch_split_btn = adw::SplitButton::builder()
        .label("Launch Game")
        .hexpand(true)
        .visible(false)
        .build();
    launch_split_btn.add_css_class("suggested-action");

    launch_box.append(&launch_simple_btn);
    launch_box.append(&launch_split_btn);
    sidebar_box.append(&launch_box);

    // Shared state: which executable and game are launched when the main
    // button is pressed.  Updated whenever the active game changes so the
    // single-shot click handlers (connected once) always see the latest values.
    let launch_default_exe: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let launch_current_game: Rc<RefCell<Option<Game>>> = Rc::new(RefCell::new(None));

    // Wire up the plain-button click
    {
        let game_rc = Rc::clone(&launch_current_game);
        let exe_rc = Rc::clone(&launch_default_exe);
        launch_simple_btn.connect_clicked(move |_| {
            let game = game_rc.borrow();
            let exe = exe_rc.borrow();
            if let (Some(g), Some(e)) = (game.as_ref(), exe.as_ref()) {
                do_launch_game(g, e);
            }
        });
    }

    // Wire up the split-button main-click
    {
        let game_rc = Rc::clone(&launch_current_game);
        let exe_rc = Rc::clone(&launch_default_exe);
        launch_split_btn.connect_clicked(move |_| {
            let game = game_rc.borrow();
            let exe = exe_rc.borrow();
            if let (Some(g), Some(e)) = (game.as_ref(), exe.as_ref()) {
                do_launch_game(g, e);
            }
        });
    }

    // Initial population of the launch button based on the current game
    {
        let cfg = config.borrow();
        let preferred = cfg
            .current_game()
            .and_then(|g| cfg.preferred_executables.get(&g.id))
            .cloned();
        update_launch_ui(
            &launch_simple_btn,
            &launch_split_btn,
            cfg.current_game(),
            preferred.as_deref(),
            &launch_default_exe,
            &launch_current_game,
            &Rc::clone(&config),
        );
    }


    // ── Navigation section ────────────────────────────────────────────────
    sidebar_box.append(&make_section_label("Navigation"));

    let nav_list = gtk4::ListBox::new();
    nav_list.add_css_class("boxed-list");
    nav_list.set_selection_mode(gtk4::SelectionMode::Single);
    nav_list.set_margin_start(12);
    nav_list.set_margin_end(12);
    nav_list.set_margin_bottom(12);

    for (name, icon) in &[
        ("Library", "applications-games-symbolic"),
        ("Load Order", "format-justify-left-symbolic"),
        ("Downloads", "folder-download-symbolic"),
        ("Preferences", "preferences-system-symbolic"),
    ] {
        let row = adw::ActionRow::builder()
            .title(*name)
            .activatable(true)
            .build();
        let img = gtk4::Image::from_icon_name(*icon);
        row.add_prefix(&img);
        nav_list.append(&row);
    }

    sidebar_box.append(&nav_list);

    // ── Stats section ─────────────────────────────────────────────────────
    sidebar_box.append(&make_section_label("Stats"));

    let stats_list = gtk4::ListBox::new();
    stats_list.add_css_class("boxed-list");
    stats_list.set_selection_mode(gtk4::SelectionMode::None);
    stats_list.set_margin_start(12);
    stats_list.set_margin_end(12);
    stats_list.set_margin_bottom(12);

    let installed_label = gtk4::Label::new(Some("0"));
    installed_label.add_css_class("dim-label");
    let installed_row = adw::ActionRow::builder().title("Installed").build();
    installed_row.add_suffix(&installed_label);

    let enabled_label = gtk4::Label::new(Some("0"));
    enabled_label.add_css_class("dim-label");
    let enabled_row = adw::ActionRow::builder().title("Enabled").build();
    enabled_row.add_suffix(&enabled_label);

    stats_list.append(&installed_row);
    stats_list.append(&enabled_row);
    sidebar_box.append(&stats_list);

    refresh_stats(&config.borrow(), &installed_label, &enabled_label);

    // Refresh stats every 2 seconds so the counts stay current after mod
    // installs, uninstalls, enables, and disables.
    {
        let installed_t = installed_label.clone();
        let enabled_t = enabled_label.clone();
        let config_t = Rc::clone(&config);
        glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
            refresh_stats(&config_t.borrow(), &installed_t, &enabled_t);
            glib::ControlFlow::Continue
        });
    }

    // Re-scan executables every 3 seconds so the launch button picks up newly
    // deployed script-extender loaders (e.g. SKSE installed as a mod and
    // then deployed to the game root via symlink).
    {
        let simple_btn_t = launch_simple_btn.clone();
        let split_btn_t = launch_split_btn.clone();
        let default_exe_t = Rc::clone(&launch_default_exe);
        let current_game_t = Rc::clone(&launch_current_game);
        let config_t = Rc::clone(&config);
        glib::timeout_add_local(std::time::Duration::from_secs(3), move || {
            let (game_clone, preferred) = {
                let cfg = config_t.borrow();
                let g = cfg.current_game().cloned();
                let p = g
                    .as_ref()
                    .and_then(|g| cfg.preferred_executables.get(&g.id))
                    .cloned();
                (g, p)
            };
            update_launch_ui(
                &simple_btn_t,
                &split_btn_t,
                game_clone.as_ref(),
                preferred.as_deref(),
                &default_exe_t,
                &current_game_t,
                &Rc::clone(&config_t),
            );
            glib::ControlFlow::Continue
        });
    }

    let sidebar_scroll = gtk4::ScrolledWindow::new();
    sidebar_scroll.set_vexpand(true);
    sidebar_scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);
    sidebar_scroll.set_child(Some(&sidebar_box));

    sidebar_toolbar.set_content(Some(&sidebar_scroll));

    let sidebar_page = adw::NavigationPage::builder()
        .title("Linkmm")
        .child(&sidebar_toolbar)
        .build();
    split_view.set_sidebar(Some(&sidebar_page));

    // ── Content area ──────────────────────────────────────────────────────

    let content_stack = gtk4::Stack::new();
    content_stack.set_transition_type(gtk4::StackTransitionType::None);
    content_stack.set_vexpand(true);
    content_stack.set_hexpand(true);

    let current_game = {
        let cfg = config.borrow();
        cfg.current_game().cloned()
    };

    // Library
    let library_widget: gtk4::Widget = match &current_game {
        Some(g) => library::build_library_page(g, Rc::clone(&config)),
        None => build_no_game_page(
            "No Game Selected",
            "Select or add a game to manage its mods.",
        ),
    };
    content_stack.add_named(&library_widget, Some("library"));

    // Load Order
    let load_order_widget = load_order::build_load_order_page(current_game.as_ref());
    content_stack.add_named(&load_order_widget, Some("load_order"));

    // Downloads
    let downloads_widget =
        downloads::build_downloads_page(current_game.as_ref(), Rc::clone(&config));
    content_stack.add_named(&downloads_widget, Some("downloads"));

    let content_page = adw::NavigationPage::builder()
        .title("Library")
        .child(&content_stack)
        .build();
    split_view.set_content(Some(&content_page));

    // Allow mobile-like narrow layouts: collapse the sidebar when the window is small.
    split_view.add_tick_callback(|sv, _| {
        let should_collapse = sv.width() < 860;
        if sv.is_collapsed() != should_collapse {
            sv.set_collapsed(should_collapse);
            if should_collapse {
                sv.set_show_content(true);
            }
        }
        glib::ControlFlow::Continue
    });

    window.set_content(Some(&split_view));

    // Select Library by default
    nav_list.select_row(nav_list.row_at_index(NAV_LIBRARY).as_ref());

    // ── Navigation signal ─────────────────────────────────────────────────
    {
        let content_stack_c = content_stack.clone();
        let content_page_c = content_page.clone();
        let config_c = Rc::clone(&config);
        let window_c = window.clone();
        let nav_list_c = nav_list.clone();

        nav_list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let game_info = {
                let cfg = config_c.borrow();
                cfg.current_game().cloned()
            };
            match row.index() {
                NAV_LIBRARY => {
                    let new_library: gtk4::Widget = match &game_info {
                        Some(g) => library::build_library_page(g, Rc::clone(&config_c)),
                        None => build_no_game_page(
                            "No Game Selected",
                            "Select or add a game to manage its mods.",
                        ),
                    };
                    if let Some(old) = content_stack_c.child_by_name("library") {
                        content_stack_c.remove(&old);
                    }
                    content_stack_c.add_named(&new_library, Some("library"));
                    content_page_c.set_title("Library");
                    content_stack_c.set_visible_child_name("library");
                }
                NAV_LOAD_ORDER => {
                    let new_load_order = load_order::build_load_order_page(game_info.as_ref());
                    if let Some(old) = content_stack_c.child_by_name("load_order") {
                        content_stack_c.remove(&old);
                    }
                    content_stack_c.add_named(&new_load_order, Some("load_order"));
                    content_page_c.set_title("Load Order");
                    content_stack_c.set_visible_child_name("load_order");
                }
                NAV_DOWNLOADS => {
                    let new_downloads =
                        downloads::build_downloads_page(game_info.as_ref(), Rc::clone(&config_c));
                    if let Some(old) = content_stack_c.child_by_name("downloads") {
                        content_stack_c.remove(&old);
                    }
                    content_stack_c.add_named(&new_downloads, Some("downloads"));
                    content_page_c.set_title("Downloads");
                    content_stack_c.set_visible_child_name("downloads");
                }
                NAV_PREFERENCES => {
                    settings::show_settings_dialog(
                        window_c.upcast_ref::<gtk4::Window>(),
                        Rc::clone(&config_c),
                    );
                    nav_list_c.select_row(nav_list_c.row_at_index(NAV_LIBRARY).as_ref());
                }
                _ => {}
            }
        });
    }

    // ── on_setup_done callback ──────────────────────────────────────────────
    let active_game_row_r = active_game_row.clone();
    let installed_r = installed_label.clone();
    let enabled_r = enabled_label.clone();
    let content_stack_r = content_stack.clone();
    let content_page_r = content_page.clone();
    let config_r = Rc::clone(&config);
    let nav_list_r = nav_list.clone();
    let launch_simple_btn_r = launch_simple_btn.clone();
    let launch_split_btn_r = launch_split_btn.clone();
    let launch_default_exe_r = Rc::clone(&launch_default_exe);
    let launch_current_game_r = Rc::clone(&launch_current_game);

    let on_setup_done_rc: Rc<dyn Fn()> = Rc::new(move || {
        let game_info = {
            let cfg = config_r.borrow();
            cfg.current_game().cloned()
        };

        if let Some(game) = &game_info {
            update_active_game_row(&active_game_row_r, Some(game.name.as_str()));
        } else {
            update_active_game_row(&active_game_row_r, None);
        }

        // Refresh the launch button for the new game
        let preferred = {
            let cfg = config_r.borrow();
            game_info
                .as_ref()
                .and_then(|g| cfg.preferred_executables.get(&g.id))
                .cloned()
        };
        update_launch_ui(
            &launch_simple_btn_r,
            &launch_split_btn_r,
            game_info.as_ref(),
            preferred.as_deref(),
            &launch_default_exe_r,
            &launch_current_game_r,
            &Rc::clone(&config_r),
        );

        // Rebuild Library page for the new game
        let new_library: gtk4::Widget = match &game_info {
            Some(g) => library::build_library_page(g, Rc::clone(&config_r)),
            None => build_no_game_page(
                "No Game Selected",
                "Select or add a game to manage its mods.",
            ),
        };
        if let Some(old) = content_stack_r.child_by_name("library") {
            content_stack_r.remove(&old);
        }
        content_stack_r.add_named(&new_library, Some("library"));

        // Rebuild Load Order page
        let new_load_order = load_order::build_load_order_page(game_info.as_ref());
        if let Some(old) = content_stack_r.child_by_name("load_order") {
            content_stack_r.remove(&old);
        }
        content_stack_r.add_named(&new_load_order, Some("load_order"));

        // Rebuild Downloads page
        let new_downloads =
            downloads::build_downloads_page(game_info.as_ref(), Rc::clone(&config_r));
        if let Some(old) = content_stack_r.child_by_name("downloads") {
            content_stack_r.remove(&old);
        }
        content_stack_r.add_named(&new_downloads, Some("downloads"));

        // Switch to Library
        content_page_r.set_title("Library");
        content_stack_r.set_visible_child_name("library");
        nav_list_r.select_row(nav_list_r.row_at_index(NAV_LIBRARY).as_ref());

        // Update stats
        refresh_stats(&config_r.borrow(), &installed_r, &enabled_r);
    });

    // ── Active-game row click → open wizard / game picker ─────────────────
    {
        let window_c = window.clone();
        let config_c = Rc::clone(&config);
        let on_finish_c = Rc::clone(&on_setup_done_rc);

        active_game_row.connect_activated(move |_| {
            let has_games = !config_c.borrow().games.is_empty();
            if !has_games {
                let f = Rc::clone(&on_finish_c);
                setup_wizard::show_setup_wizard(&window_c, Rc::clone(&config_c), move || f());
            } else {
                show_game_picker(&window_c, Rc::clone(&config_c), Rc::clone(&on_finish_c));
            }
        });
    }

    let on_setup_done_final = Rc::clone(&on_setup_done_rc);
    (window, move || on_setup_done_final())
}

// ── Helper widgets ─────────────────────────────────────────────────────────

fn make_section_label(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("heading");
    label.set_halign(gtk4::Align::Start);
    label.set_margin_start(18);
    label.set_margin_top(16);
    label.set_margin_bottom(4);
    label
}

fn update_active_game_row(row: &adw::ActionRow, name: Option<&str>) {
    match name {
        Some(n) => {
            row.set_title(n);
            row.set_subtitle("Click to switch game");
        }
        None => {
            row.set_title("No Game Selected");
            row.set_subtitle("Click to add a game");
        }
    }
}

fn build_no_game_page(title: &str, description: &str) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let status = adw::StatusPage::builder()
        .title(title)
        .description(description)
        .icon_name("applications-games-symbolic")
        .build();
    status.set_vexpand(true);
    toolbar_view.set_content(Some(&status));
    toolbar_view.upcast()
}

fn refresh_stats(
    cfg: &AppConfig,
    installed: &gtk4::Label,
    enabled: &gtk4::Label,
) {
    if let Some(game) = cfg.current_game() {
        let db = ModDatabase::load(game);
        installed.set_text(&db.mods.len().to_string());
        enabled.set_text(&db.mods.iter().filter(|m| m.enabled).count().to_string());
    } else {
        installed.set_text("0");
        enabled.set_text("0");
    }
}

// ── Launch-button helpers ──────────────────────────────────────────────────

/// Update the sidebar launch button(s) to reflect the currently active game.
///
/// * Hides both buttons when `game` is `None` or no executables are found.
/// * Shows the plain `simple_btn` when exactly one executable exists.
/// * Shows the `split_btn` (with a dropdown listing all executables) when
///   more than one executable is discovered in the game directory.
///
/// The active executable is chosen as follows:
/// * If the **game just changed** (including the initial call), the value of
///   `preferred_exe` — loaded from `AppConfig::preferred_executables` — is
///   used if that executable is still present on disk.  Otherwise the first
///   discovered executable is used.
/// * If the **same game is being refreshed** (periodic scan to pick up newly
///   deployed script-extender loaders), the current in-memory selection is
///   preserved as long as it is still valid.
///
/// When the user selects an executable from the split-button dropdown the
/// choice is immediately written to `config` and saved to disk so it
/// survives an application restart.
fn update_launch_ui(
    simple_btn: &gtk4::Button,
    split_btn: &adw::SplitButton,
    game: Option<&Game>,
    preferred_exe: Option<&str>,
    default_exe: &Rc<RefCell<Option<String>>>,
    current_game: &Rc<RefCell<Option<Game>>>,
    config: &Rc<RefCell<AppConfig>>,
) {
    // Always hide both first; we show the right one below.
    simple_btn.set_visible(false);
    split_btn.set_visible(false);

    let Some(g) = game else {
        *default_exe.borrow_mut() = None;
        *current_game.borrow_mut() = None;
        return;
    };

    let game_changed = current_game
        .borrow()
        .as_ref()
        .map(|cg| cg.id != g.id)
        .unwrap_or(true);
    *current_game.borrow_mut() = Some(g.clone());

    let exes = g.discover_executables();
    if exes.is_empty() {
        *default_exe.borrow_mut() = None;
        return;
    }

    // Choose which exe to show as the active default:
    // • game changed → use persisted preference (if still on disk) else first exe
    // • same game    → preserve current in-memory selection if still valid
    let preferred_or_first = |pref: Option<&str>| -> String {
        pref.filter(|p| exes.iter().any(|e| e.as_str() == *p))
            .map(str::to_string)
            .unwrap_or_else(|| exes[0].clone())
    };
    let chosen: String = if game_changed {
        preferred_or_first(preferred_exe)
    } else {
        let current = default_exe.borrow().clone();
        current
            .filter(|e| exes.iter().any(|x| x == e))
            .unwrap_or_else(|| preferred_or_first(preferred_exe))
    };

    *default_exe.borrow_mut() = Some(chosen.clone());

    if exes.len() == 1 {
        simple_btn.set_label(&exes[0]);
        simple_btn.set_visible(true);
    } else {
        // Multiple executables: build a popover listing all of them.
        let popover = gtk4::Popover::new();
        let list = gtk4::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_margin_start(4);
        list.set_margin_end(4);
        list.set_margin_top(4);
        list.set_margin_bottom(4);

        for exe in &exes {
            let row = adw::ActionRow::builder()
                .title(exe.as_str())
                .activatable(true)
                .build();

            let exe_c = exe.clone();
            let game_id_c = g.id.clone();
            let game_c = g.clone();
            let default_exe_c = Rc::clone(default_exe);
            let split_btn_c = split_btn.clone();
            let popover_c = popover.clone();
            let config_c = Rc::clone(config);
            row.connect_activated(move |_| {
                // Update the default exe shown on the button.
                *default_exe_c.borrow_mut() = Some(exe_c.clone());
                split_btn_c.set_label(&exe_c);
                popover_c.popdown();
                // Persist the user's choice so it survives a restart.
                {
                    let mut cfg = config_c.borrow_mut();
                    cfg.preferred_executables
                        .insert(game_id_c.clone(), exe_c.clone());
                    cfg.save();
                }
                do_launch_game(&game_c, &exe_c);
            });

            list.append(&row);
        }

        popover.set_child(Some(&list));
        split_btn.set_popover(Some(&popover));
        split_btn.set_label(&chosen);
        split_btn.set_visible(true);
    }
}

/// Attempt to launch `exe_name` for `game`, logging a warning on failure.
fn do_launch_game(game: &Game, exe_name: &str) {
    if let Err(e) = crate::core::steam::launch_game_executable(game, exe_name) {
        log::warn!("Could not launch {} for {}: {e}", exe_name, game.name);
    }
}

// ── Game picker dialog ─────────────────────────────────────────────────────

fn show_game_picker(
    parent: &adw::ApplicationWindow,
    config: Rc<RefCell<AppConfig>>,
    on_game_changed: Rc<dyn Fn()>,
) {
    let dialog = adw::Window::builder()
        .title("Switch Game")
        .modal(true)
        .transient_for(parent)
        .default_width(480)
        .default_height(400)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content_box.set_margin_start(24);
    content_box.set_margin_end(24);
    content_box.set_margin_top(12);
    content_box.set_margin_bottom(12);

    let game_list = gtk4::ListBox::new();
    game_list.add_css_class("boxed-list");
    game_list.set_selection_mode(gtk4::SelectionMode::None);

    let current_id = config.borrow().current_game_id.clone();

    {
        let games = config.borrow().games.clone();
        for game in &games {
            let row = adw::ActionRow::builder()
                .title(&game.name)
                .subtitle(game.root_path.to_string_lossy().as_ref())
                .activatable(true)
                .build();

            if current_id.as_deref() == Some(&game.id) {
                let check = gtk4::Image::from_icon_name("object-select-symbolic");
                row.add_suffix(&check);
            }

            let game_id = game.id.clone();
            let config_c = Rc::clone(&config);
            let dialog_c = dialog.clone();
            let on_changed_c = Rc::clone(&on_game_changed);
            row.connect_activated(move |_| {
                config_c.borrow_mut().current_game_id = Some(game_id.clone());
                config_c.borrow().save();
                dialog_c.destroy();
                on_changed_c();
            });

            game_list.append(&row);
        }
    }

    content_box.append(&game_list);

    let add_btn = gtk4::Button::with_label("Add New Game\u{2026}");
    add_btn.add_css_class("suggested-action");
    add_btn.set_halign(gtk4::Align::Center);
    add_btn.set_margin_top(8);

    let parent_c = parent.clone();
    let config_c = Rc::clone(&config);
    let dialog_c = dialog.clone();
    let on_changed_c = Rc::clone(&on_game_changed);
    add_btn.connect_clicked(move |_| {
        dialog_c.destroy();
        let f = Rc::clone(&on_changed_c);
        setup_wizard::show_setup_wizard(&parent_c, Rc::clone(&config_c), move || f());
    });
    content_box.append(&add_btn);

    toolbar_view.set_content(Some(&content_box));
    dialog.set_content(Some(&toolbar_view));
    dialog.present();
}

// ── About action ───────────────────────────────────────────────────────────

fn register_about_action(app: &libadwaita::Application, window: &adw::ApplicationWindow) {
    let action = gio::SimpleAction::new("about", None);
    let window_c = window.clone();
    action.connect_activate(move |_, _| {
        show_about_dialog(window_c.upcast_ref::<gtk4::Window>());
    });
    app.add_action(&action);
}

fn show_about_dialog(parent: &gtk4::Window) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Linkmm")
        .application_icon("applications-games-symbolic")
        .developer_name("Linkmm Contributors")
        .version(env!("CARGO_PKG_VERSION"))
        .website("https://github.com/sachesi/linkmm")
        .issue_url("https://github.com/sachesi/linkmm/issues")
        .license_type(gtk4::License::Gpl30)
        .build();
    dialog.present(Some(parent));
}

// ── NXM protocol handler ──────────────────────────────────────────────────

/// Handle an `nxm://` URL received from the browser.
///
/// Parses the URL, fetches the download link from the Nexus API, downloads the
/// file to the configured downloads directory, and shows a toast on the active
/// window.
pub fn handle_nxm_url(app: &libadwaita::Application, url: &str) {
    use crate::core::download;
    use crate::core::download_state;
    use crate::core::nexus::NexusClient;
    use crate::core::nxm::NxmUrl;

    let config = AppConfig::load_or_default();
    let Some(api_key) = config.nexus_api_key.clone() else {
        log::error!("Cannot handle NXM URL: no API key configured");
        if let Some(window) = app.active_window() {
            show_nxm_toast(&window, "Set your NexusMods API key in Preferences first.");
        }
        return;
    };

    let nxm = match NxmUrl::parse(url) {
        Ok(n) => n,
        Err(e) => {
            log::error!("Failed to parse NXM URL: {e}");
            if let Some(window) = app.active_window() {
                show_nxm_toast(&window, &format!("Invalid NXM link: {e}"));
            }
            return;
        }
    };

    log::info!(
        "Handling NXM URL: game={}, mod={}, file={}",
        nxm.game_domain,
        nxm.mod_id,
        nxm.file_id
    );

    let managed_game_id = config
        .games
        .iter()
        .find(|g| g.kind.nexus_game_id() == nxm.game_domain)
        .map(|g| g.id.as_str())
        .or(config.current_game_id.as_deref());
    let downloads_dir = config.downloads_dir(managed_game_id);
    if let Err(e) = std::fs::create_dir_all(&downloads_dir) {
        log::error!("Failed to create downloads directory: {e}");
        return;
    }

    if let Some(window) = app.active_window() {
        show_nxm_toast(
            &window,
            &format!("Starting download for mod {}…", nxm.mod_id),
        );
    }

    let (tx, rx) = mpsc::channel::<Result<String, String>>();

    std::thread::spawn(move || {
        let result = (|| -> Result<String, String> {
            let client = NexusClient::new(&api_key);

            // Get file info to determine the file name
            let files = client.get_mod_files(&nxm.game_domain, nxm.mod_id as u32)?;
            let file_info = files.iter().find(|f| f.file_id == nxm.file_id);
            let raw_name = match file_info {
                Some(f) => f.file_name.clone(),
                None => format!("mod_{}_{}.zip", nxm.mod_id, nxm.file_id),
            };

            // Sanitize filename: strip path components, reject traversal
            let file_name = std::path::Path::new(&raw_name)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("mod_{}_{}.zip", nxm.mod_id, nxm.file_id));
            if file_name.contains("..") || file_name.starts_with('.') {
                return Err("Invalid filename from server".to_string());
            }

            let dest_path = downloads_dir.join(&file_name);
            if dest_path.exists() {
                if let Err(e) =
                    write_nxm_download_metadata(&dest_path, &nxm.game_domain, nxm.mod_id as u32)
                {
                    log::warn!("Failed to update NXM metadata for {}: {e}", file_name);
                }
                return Ok(format!("{file_name} (already downloaded)"));
            }

            // Get download link using NXM key/expires if available, otherwise
            // fall back to the premium-only direct API
            let links = match (&nxm.key, &nxm.expires) {
                (Some(key), Some(expires)) => client.get_download_links_nxm(
                    &nxm.game_domain,
                    nxm.mod_id as u32,
                    nxm.file_id,
                    key,
                    expires,
                )?,
                _ => client.get_download_links(&nxm.game_domain, nxm.mod_id as u32, nxm.file_id)?,
            };

            let (_cdn, url) = links
                .first()
                .ok_or_else(|| "No download links available".to_string())?;

            let download_id = download_state::set_active(file_name.clone());
            let download_result = download::download_file(url, &dest_path, |downloaded, total| {
                download_state::update_progress(download_id, downloaded, total);
                !download_state::is_cancel_requested(download_id)
            });
            download_state::clear_active(download_id);
            download_result?;

            if let Err(e) =
                write_nxm_download_metadata(&dest_path, &nxm.game_domain, nxm.mod_id as u32)
            {
                log::warn!("Failed to write NXM metadata for {}: {e}", file_name);
            }

            Ok(file_name)
        })();
        let _ = tx.send(result);
    });

    // Poll for completion and show toast
    let app_c = app.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        match rx.try_recv() {
            Ok(Ok(file_name)) => {
                log::info!("NXM download complete: {file_name}");
                if let Some(window) = app_c.active_window() {
                    show_nxm_toast(&window, &format!("Downloaded: {file_name}"));
                }
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                if let Some(window) = app_c.active_window() {
                    if e == download::DOWNLOAD_CANCELLED_ERROR {
                        log::info!("NXM download cancelled");
                        show_nxm_toast(&window, "Download cancelled");
                    } else {
                        log::error!("NXM download failed: {e}");
                        show_nxm_toast(&window, &format!("Download failed: {e}"));
                    }
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn show_nxm_toast(window: &gtk4::Window, message: &str) {
    // Try to find a ToastOverlay in the window hierarchy
    if let Some(child) = window.child() {
        walk_for_toast_overlay(&child, message);
    }
    log::info!("NXM: {message}");
}

fn walk_for_toast_overlay(widget: &gtk4::Widget, message: &str) {
    if let Ok(overlay) = widget.clone().downcast::<adw::ToastOverlay>() {
        let toast = adw::Toast::new(message);
        toast.set_timeout(4);
        overlay.add_toast(toast);
        return;
    }
    // Try first child recursively
    if let Some(child) = widget.first_child() {
        walk_for_toast_overlay(&child, message);
    }
}

fn nxm_metadata_path_for_archive(archive_path: &std::path::Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}.nxm.json", archive_path.to_string_lossy()))
}

fn write_nxm_download_metadata(
    archive_path: &std::path::Path,
    game_domain: &str,
    mod_id: u32,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "game_domain": game_domain,
        "mod_id": mod_id
    });
    let path = nxm_metadata_path_for_archive(archive_path);
    let body = serde_json::to_string_pretty(&payload)
        .map_err(|e| format!("Failed to serialize NXM metadata: {e}"))?;
    std::fs::write(&path, body).map_err(|e| format!("Failed to write {}: {e}", path.display()))
}
