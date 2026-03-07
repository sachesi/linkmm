use std::cell::RefCell;
use std::rc::Rc;

use gio;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::mods::ModDatabase;

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
const NAV_PREFERENCES: i32 = 2;

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
    // The row itself handles activation; the ListBox doesn't need selection
    active_game_list.set_selection_mode(gtk4::SelectionMode::None);

    let active_game_row = adw::ActionRow::builder()
        .activatable(true)
        .build();
    let game_icon = gtk4::Image::from_icon_name("applications-games-symbolic");
    active_game_row.add_prefix(&game_icon);

    {
        let cfg = config.borrow();
        update_active_game_row(&active_game_row, cfg.current_game().map(|g| g.name.as_str()));
    }

    active_game_list.append(&active_game_row);
    sidebar_box.append(&active_game_list);

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

    let conflicts_label = gtk4::Label::new(Some("0"));
    conflicts_label.add_css_class("dim-label");
    let conflicts_row = adw::ActionRow::builder().title("Conflicts").build();
    conflicts_row.add_suffix(&conflicts_label);

    stats_list.append(&installed_row);
    stats_list.append(&enabled_row);
    stats_list.append(&conflicts_row);
    sidebar_box.append(&stats_list);

    // Populate stats for the current game immediately
    refresh_stats(
        &config.borrow(),
        &installed_label,
        &enabled_label,
        &conflicts_label,
    );

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

    // A Stack holds one page per navigation destination.
    // Pages that require a game are initially placeholders and get replaced
    // when a game is added / selected.
    let content_stack = gtk4::Stack::new();
    content_stack.set_transition_type(gtk4::StackTransitionType::None);
    content_stack.set_vexpand(true);
    content_stack.set_hexpand(true);

    // Build initial pages (using current game if any)
    let current_game = {
        let cfg = config.borrow();
        cfg.current_game().cloned()
    };

    // Library
    let library_widget: gtk4::Widget = match &current_game {
        Some(g) => library::build_library_page(g, Rc::clone(&config)),
        None => build_no_game_page("No Game Selected", "Select or add a game to manage its mods."),
    };
    content_stack.add_named(&library_widget, Some("library"));

    // Load Order
    let load_order_widget = load_order::build_load_order_page(current_game.as_ref());
    content_stack.add_named(&load_order_widget, Some("load_order"));

    let content_page = adw::NavigationPage::builder()
        .title("Library")
        .child(&content_stack)
        .build();
    split_view.set_content(Some(&content_page));

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
            match row.index() {
                NAV_LIBRARY => {
                    content_page_c.set_title("Library");
                    content_stack_c.set_visible_child_name("library");
                }
                NAV_LOAD_ORDER => {
                    content_page_c.set_title("Load Order");
                    content_stack_c.set_visible_child_name("load_order");
                }
                NAV_PREFERENCES => {
                    // Open settings as a dialog; revert selection to Library
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

    // ── on_setup_done callback (defined before signals that need it) ──────────
    let active_game_row_r = active_game_row.clone();
    let installed_r = installed_label.clone();
    let enabled_r = enabled_label.clone();
    let conflicts_r = conflicts_label.clone();
    let content_stack_r = content_stack.clone();
    let content_page_r = content_page.clone();
    let config_r = Rc::clone(&config);
    let nav_list_r = nav_list.clone();

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

        // Switch to Library
        content_page_r.set_title("Library");
        content_stack_r.set_visible_child_name("library");
        nav_list_r.select_row(nav_list_r.row_at_index(NAV_LIBRARY).as_ref());

        // Update stats
        refresh_stats(
            &config_r.borrow(),
            &installed_r,
            &enabled_r,
            &conflicts_r,
        );
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
    conflicts: &gtk4::Label,
) {
    if let Some(game) = cfg.current_game() {
        let db = ModDatabase::load(game);
        installed.set_text(&db.mods.len().to_string());
        enabled.set_text(&db.mods.iter().filter(|m| m.enabled).count().to_string());
    } else {
        installed.set_text("0");
        enabled.set_text("0");
    }
    conflicts.set_text("0");
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

    // "Add New Game" button
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

