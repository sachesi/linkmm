use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;

use gio;
use gtk4::gdk;
use gtk4::graphene;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::Game;
use crate::core::mods::{Mod, ModDatabase, ModManager};
use crate::ui::drag_autoscroll::{
    DEFAULT_TICK_MS, EdgeAutoScrollConfig, EdgeAutoScrollState, attach_viewport_drag_autoscroll,
    stop_drag_autoscroll,
};

const TOAST_TIMEOUT_SECONDS: u32 = 3;
const STATUS_POPUP_HIDE_DELAY_MS: u64 = 900;

#[derive(Debug, Clone, Default)]
struct ConflictState {
    overwrites: bool,
    overwritten: bool,
    files: BTreeSet<String>,
    conflict_mods_by_file: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone)]
struct ViewportAnchor {
    item_key: String,
    align_ratio: f64,
}

/// Build the full Library page for `game`.
pub fn build_library_page(game: &Game, config: Rc<RefCell<AppConfig>>) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let title_widget = adw::WindowTitle::new("Library", "");
    header.set_title_widget(Some(&title_widget));

    let search_entry = gtk4::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Search mods"));
    search_entry.set_width_chars(24);
    header.pack_start(&search_entry);

    // Deploy button – applies all enabled mods by (re)linking their files
    let deploy_btn = gtk4::Button::with_label("Deploy");
    deploy_btn.add_css_class("suggested-action");
    deploy_btn.set_tooltip_text(Some(
        "Apply all enabled mods by linking their files into the game directory",
    ));
    header.pack_end(&deploy_btn);

    toolbar_view.add_top_bar(&header);

    let content_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_container.set_vexpand(true);

    let status_revealer = gtk4::Revealer::new();
    status_revealer.set_transition_type(gtk4::RevealerTransitionType::SlideDown);
    status_revealer.set_reveal_child(false);

    let status_card = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    status_card.add_css_class("card");
    status_card.set_margin_bottom(4);
    status_card.set_margin_top(4);
    status_card.set_margin_start(4);
    status_card.set_margin_end(4);

    let status_label = gtk4::Label::new(None);
    status_label.set_xalign(0.0);
    status_label.add_css_class("dim-label");
    status_label.set_margin_top(8);
    status_label.set_margin_start(8);
    status_label.set_margin_end(8);

    let status_progress = gtk4::ProgressBar::new();
    status_progress.set_show_text(true);
    status_progress.set_margin_start(8);
    status_progress.set_margin_end(8);
    status_progress.set_margin_bottom(8);

    status_card.append(&status_label);
    status_card.append(&status_progress);
    status_revealer.set_child(Some(&status_card));
    content_container.append(&status_revealer);

    let list_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    list_container.set_vexpand(true);
    content_container.append(&list_container);

    let game_rc = Rc::new(game.clone());
    let search_query = Rc::new(RefCell::new(String::new()));
    let selected_mod_id = Rc::new(RefCell::new(None::<String>));
    let pending_viewport_anchor = Rc::new(RefCell::new(None::<ViewportAnchor>));
    let drag_autoscroll = Rc::new(RefCell::new(DragAutoScrollState::default()));

    refresh_library_content_with_search(
        &list_container,
        &game_rc,
        Rc::clone(&config),
        &search_query.borrow(),
        Rc::clone(&search_query),
        Rc::clone(&selected_mod_id),
        Rc::clone(&pending_viewport_anchor),
        Rc::clone(&drag_autoscroll),
        true,
    );

    toolbar_view.set_content(Some(&content_container));

    {
        let container_c = list_container.clone();
        let game_c = Rc::clone(&game_rc);
        let config_c = Rc::clone(&config);
        let search_c = Rc::clone(&search_query);
        let selected_c = Rc::clone(&selected_mod_id);
        let anchor_c = Rc::clone(&pending_viewport_anchor);
        let drag_scroll_c = Rc::clone(&drag_autoscroll);
        search_entry.connect_search_changed(move |entry| {
            *search_c.borrow_mut() = entry.text().to_string();
            refresh_library_content_with_search(
                &container_c,
                &game_c,
                Rc::clone(&config_c),
                &search_c.borrow(),
                Rc::clone(&search_c),
                Rc::clone(&selected_c),
                Rc::clone(&anchor_c),
                Rc::clone(&drag_scroll_c),
                false,
            );
        });
    }

    // Wire Deploy button: undeploy everything, then deploy all enabled mods
    {
        let game_c = Rc::clone(&game_rc);
        let container_c = list_container.clone();
        let config_c = Rc::clone(&config);
        let search_c = Rc::clone(&search_query);
        let selected_c = Rc::clone(&selected_mod_id);
        let anchor_c = Rc::clone(&pending_viewport_anchor);
        let drag_scroll_c = Rc::clone(&drag_autoscroll);
        let search_entry_c = search_entry.clone();
        let deploy_btn_c = deploy_btn.clone();
        let status_label_c = status_label.clone();
        let status_progress_c = status_progress.clone();
        let status_revealer_c = status_revealer.clone();
        deploy_btn.connect_clicked(move |btn| {
            set_library_busy(&search_entry_c, &deploy_btn_c, &container_c, true);
            status_revealer_c.set_reveal_child(true);
            status_label_c.set_text("Preparing deployment…");
            status_progress_c.set_fraction(0.0);
            status_progress_c.set_text(Some("0%"));

            let game_c = Rc::clone(&game_c);
            let container_c = container_c.clone();
            let config_c = Rc::clone(&config_c);
            let search_c = Rc::clone(&search_c);
            let selected_c = Rc::clone(&selected_c);
            let search_entry_c = search_entry_c.clone();
            let deploy_btn_c = deploy_btn_c.clone();
            let status_label_c = status_label_c.clone();
            let status_progress_c = status_progress_c.clone();
            let status_revealer_c = status_revealer_c.clone();
            let btn = btn.clone();
            let anchor_for_timeout = Rc::clone(&anchor_c);
            let drag_scroll_for_timeout = Rc::clone(&drag_scroll_c);

            status_label_c.set_text("Rebuilding deployment from enabled mod set…");
            status_progress_c.pulse();
            status_progress_c.set_text(Some("Working…"));

            let (tx, rx) = mpsc::channel::<Result<usize, String>>();
            let game_bg = (*game_c).clone();
            thread::spawn(move || {
                let db = ModDatabase::load(&game_bg);
                let enabled_count = db.mods.iter().filter(|m| m.enabled).count();
                let result = ModManager::rebuild_all(&game_bg)
                    .map(|()| enabled_count)
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
            });

            gtk4::glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
                match rx.try_recv() {
                    Ok(result) => {
                        let msg = match result {
                            Ok(enabled_count) => format!("Deployed {enabled_count} mod(s)"),
                            Err(e) => {
                                log::error!("Deploy error: {e}");
                                format!("Deploy failed: {e}")
                            }
                        };
                        status_label_c.set_text(&msg);
                        status_progress_c.set_fraction(1.0);
                        status_progress_c.set_text(Some("100%"));
                        set_library_busy(&search_entry_c, &deploy_btn_c, &container_c, false);
                        hide_status_popup_later(status_revealer_c.clone());
                        show_toast(btn.upcast_ref(), &msg);
                        refresh_library_content_with_search(
                            &container_c,
                            &game_c,
                            Rc::clone(&config_c),
                            &search_c.borrow(),
                            Rc::clone(&search_c),
                            Rc::clone(&selected_c),
                            Rc::clone(&anchor_for_timeout),
                            Rc::clone(&drag_scroll_for_timeout),
                            true,
                        );
                        gtk4::glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        status_progress_c.pulse();
                        gtk4::glib::ControlFlow::Continue
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        status_label_c.set_text("Deploy failed: background task disconnected");
                        status_progress_c.set_fraction(1.0);
                        status_progress_c.set_text(Some("Failed"));
                        set_library_busy(&search_entry_c, &deploy_btn_c, &container_c, false);
                        hide_status_popup_later(status_revealer_c.clone());
                        gtk4::glib::ControlFlow::Break
                    }
                }
            });
        });
    }

    toolbar_view.upcast()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn find_existing_library_view(
    container: &gtk4::Box,
) -> Option<(gtk4::Stack, gtk4::ListBox, gtk4::ScrolledWindow)> {
    let stack = container
        .first_child()
        .and_then(|child| child.downcast::<gtk4::Stack>().ok())?;
    let list_page = stack.child_by_name("list")?;
    let scrolled = list_page.downcast::<gtk4::ScrolledWindow>().ok()?;
    let clamp = scrolled.child()?.downcast::<adw::Clamp>().ok()?;
    let list_box = clamp.child()?.downcast::<gtk4::ListBox>().ok()?;
    Some((stack, list_box, scrolled))
}

fn ensure_library_view(
    container: &gtk4::Box,
    drag_autoscroll: Rc<RefCell<EdgeAutoScrollState>>,
) -> (gtk4::Stack, gtk4::ListBox, gtk4::ScrolledWindow) {
    if let Some(view) = find_existing_library_view(container) {
        return view;
    }

    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(900);
    clamp.set_child(Some(&list_box));
    clamp.set_margin_top(12);
    clamp.set_margin_bottom(12);
    clamp.set_margin_start(12);
    clamp.set_margin_end(12);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    scrolled.set_child(Some(&clamp));
    attach_viewport_drag_autoscroll(
        &scrolled,
        drag_autoscroll,
        EdgeAutoScrollConfig::default(),
        DEFAULT_TICK_MS,
    );

    let stack = gtk4::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(&scrolled, Some("list"));

    container.append(&stack);
    (stack, list_box, scrolled)
}

fn set_library_status(stack: &gtk4::Stack, status: &adw::StatusPage) {
    if let Some(existing) = stack.child_by_name("status") {
        stack.remove(&existing);
    }
    stack.add_named(status, Some("status"));
    stack.set_visible_child_name("status");
}

fn library_row_key(mod_id: &str) -> String {
    format!("library:{mod_id}")
}

fn find_row_by_key(list_box: &gtk4::ListBox, key: &str) -> Option<gtk4::Widget> {
    let mut child = list_box.first_child();
    while let Some(widget) = child {
        if widget.widget_name() == key {
            return Some(widget);
        }
        child = widget.next_sibling();
    }
    None
}

fn widget_y_in_scrolled(widget: &gtk4::Widget, scrolled: &gtk4::ScrolledWindow) -> Option<f64> {
    widget
        .compute_point(scrolled, &graphene::Point::new(0.0, 0.0))
        .map(|point| point.y() as f64)
}

fn refresh_library_content_with_search(
    container: &gtk4::Box,
    game: &Rc<Game>,
    config: Rc<RefCell<AppConfig>>,
    search_query: &str,
    search_state: Rc<RefCell<String>>,
    selected_mod_id: Rc<RefCell<Option<String>>>,
    pending_viewport_anchor: Rc<RefCell<Option<ViewportAnchor>>>,
    drag_autoscroll: Rc<RefCell<DragAutoScrollState>>,
    do_scan: bool,
) {
    let (stack, list_box, scrolled) = ensure_library_view(container, Rc::clone(&drag_autoscroll));
    let previous_scroll = {
        let adj = scrolled.vadjustment();
        (adj.value(), adj.upper(), adj.page_size())
    };

    let mut db = ModDatabase::load(game);
    if do_scan {
        db.scan_mods_dir(game);
        db.save(game);
    }

    if let Some(selected) = selected_mod_id.borrow().as_ref()
        && !db.mods.iter().any(|m| &m.id == selected)
    {
        *selected_mod_id.borrow_mut() = None;
    }

    let visible_mods: Vec<_> = db
        .mods
        .iter()
        .filter(|m| matches_query(&m.name, search_query))
        .cloned()
        .collect();

    if visible_mods.is_empty() {
        let status = if !search_query.trim().is_empty() && !db.mods.is_empty() {
            adw::StatusPage::builder()
                .title("No Matching Mods")
                .description("No installed mods match your search.")
                .icon_name("system-search-symbolic")
                .build()
        } else {
            adw::StatusPage::builder()
                .title("No Mods Installed")
                .icon_name("package-x-generic-symbolic")
                .build()
        };
        status.set_vexpand(true);
        set_library_status(&stack, &status);
        return;
    }

    if let Some(status) = stack.child_by_name("status") {
        stack.remove(&status);
    }
    stack.set_visible_child_name("list");

    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let selected = selected_mod_id.borrow().clone();
    let conflict_states = compute_conflict_states(&db.mods, selected.as_deref());

    for (idx, mod_entry) in visible_mods.iter().enumerate() {
        let row = build_mod_row(
            mod_entry,
            idx,
            visible_mods.len(),
            game,
            container,
            Rc::clone(&config),
            Rc::clone(&search_state),
            Rc::clone(&selected_mod_id),
            Rc::clone(&pending_viewport_anchor),
            Rc::clone(&drag_autoscroll),
            conflict_states.get(&mod_entry.id),
        );
        list_box.append(&row);
    }

    let anchor = pending_viewport_anchor.borrow_mut().take();
    let list_box_clone = list_box.clone();
    let scrolled_clone = scrolled.clone();
    gtk4::glib::idle_add_local_once(move || {
        let adj = scrolled_clone.vadjustment();
        let max_value = (adj.upper() - adj.page_size()).max(0.0);
        let anchored_value = anchor.and_then(|a| {
            find_row_by_key(&list_box_clone, &library_row_key(&a.item_key)).and_then(|row| {
                widget_y_in_scrolled(&row, &scrolled_clone)
                    .map(|row_y| (row_y - (adj.page_size() * a.align_ratio)).max(0.0))
            })
        });
        let target = anchored_value
            .unwrap_or(previous_scroll.0)
            .clamp(0.0, max_value);
        adj.set_value(target);
    });
}

/// Toggle interactivity for Library controls during long deploy operations.
fn set_library_busy(
    search_entry: &gtk4::SearchEntry,
    deploy_btn: &gtk4::Button,
    content_container: &gtk4::Box,
    busy: bool,
) {
    let sensitive = !busy;
    search_entry.set_sensitive(sensitive);
    deploy_btn.set_sensitive(sensitive);
    content_container.set_sensitive(sensitive);
}

fn hide_status_popup_later(status_revealer: gtk4::Revealer) {
    gtk4::glib::timeout_add_local_once(
        std::time::Duration::from_millis(STATUS_POPUP_HIDE_DELAY_MS),
        move || {
            status_revealer.set_reveal_child(false);
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn build_mod_row(
    mod_entry: &Mod,
    idx: usize,
    total: usize,
    game: &Rc<Game>,
    container: &gtk4::Box,
    config: Rc<RefCell<AppConfig>>,
    search_state: Rc<RefCell<String>>,
    selected_mod_id: Rc<RefCell<Option<String>>>,
    pending_viewport_anchor: Rc<RefCell<Option<ViewportAnchor>>>,
    drag_autoscroll: Rc<RefCell<DragAutoScrollState>>,
    conflict_state: Option<&ConflictState>,
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title(&mod_entry.name)
        .active(mod_entry.enabled)
        .build();
    row.set_widget_name(&library_row_key(&mod_entry.id));

    // Subtitle: version and Nexus source indicator
    let subtitle = match (&mod_entry.version, mod_entry.installed_from_nexus) {
        (Some(v), true) => format!("{v} · From Nexus Mods"),
        (Some(v), false) => v.clone(),
        (None, true) => "From Nexus Mods".to_string(),
        (None, false) => String::new(),
    };
    if !subtitle.is_empty() {
        row.set_subtitle(&subtitle);
    }

    // Drag handle + index prefix
    let drag_handle = gtk4::Image::from_icon_name("list-drag-handle-symbolic");
    drag_handle.add_css_class("dim-label");
    drag_handle.set_tooltip_text(Some("Drag to reorder"));
    row.add_prefix(&drag_handle);

    let index_label = gtk4::Label::new(Some(&format!("{}", idx + 1)));
    index_label.add_css_class("dim-label");
    index_label.add_css_class("numeric");
    index_label.set_width_chars(3);
    row.add_prefix(&index_label);

    if let Some(state) = conflict_state {
        if state.overwritten {
            row.add_css_class("error");
        } else if state.overwrites {
            row.add_css_class("success");
        } else if !state.files.is_empty() {
            row.add_css_class("accent");
        }
    }

    let mod_id = mod_entry.id.clone();
    let game_clone = Rc::clone(game);

    row.connect_active_notify(move |switch_row| {
        let enabled = switch_row.is_active();
        if let Err(e) = ModManager::set_mod_enabled(&game_clone, &mod_id, enabled) {
            log::error!("Failed to rebuild deployment after toggle: {e}");
            switch_row.set_active(!enabled);
        }
    });

    // Move up / down
    let up_btn = gtk4::Button::new();
    up_btn.set_icon_name("go-up-symbolic");
    up_btn.set_valign(gtk4::Align::Center);
    up_btn.add_css_class("flat");
    up_btn.set_tooltip_text(Some("Move up"));
    up_btn.set_sensitive(idx > 0);

    let down_btn = gtk4::Button::new();
    down_btn.set_icon_name("go-down-symbolic");
    down_btn.set_valign(gtk4::Align::Center);
    down_btn.add_css_class("flat");
    down_btn.set_tooltip_text(Some("Move down"));
    down_btn.set_sensitive(idx + 1 < total);

    row.add_suffix(&up_btn);
    row.add_suffix(&down_btn);

    {
        let game_c = Rc::clone(game);
        let container_c = container.clone();
        let config_c = Rc::clone(&config);
        let search_c = Rc::clone(&search_state);
        let selected_c = Rc::clone(&selected_mod_id);
        let anchor_c = Rc::clone(&pending_viewport_anchor);
        let drag_scroll_c = Rc::clone(&drag_autoscroll);
        let mod_id_c = mod_entry.id.clone();
        up_btn.connect_clicked(move |_| {
            let mut db = ModDatabase::load(&game_c);
            if let Some(pos) = db.mods.iter().position(|m| m.id == mod_id_c)
                && pos > 0
            {
                db.mods.swap(pos, pos - 1);
                db.save(&game_c);
                if let Err(e) = ModManager::rebuild_all(&game_c) {
                    log::error!("Failed to rebuild deployment after reorder: {e}");
                }
                *anchor_c.borrow_mut() = Some(ViewportAnchor {
                    item_key: mod_id_c.clone(),
                    align_ratio: 0.35,
                });
                refresh_library_content_with_search(
                    &container_c,
                    &game_c,
                    Rc::clone(&config_c),
                    &search_c.borrow(),
                    Rc::clone(&search_c),
                    Rc::clone(&selected_c),
                    Rc::clone(&anchor_c),
                    Rc::clone(&drag_scroll_c),
                    false,
                );
            }
        });
    }

    {
        let game_c = Rc::clone(game);
        let container_c = container.clone();
        let config_c = Rc::clone(&config);
        let search_c = Rc::clone(&search_state);
        let selected_c = Rc::clone(&selected_mod_id);
        let anchor_c = Rc::clone(&pending_viewport_anchor);
        let drag_scroll_c = Rc::clone(&drag_autoscroll);
        let mod_id_c = mod_entry.id.clone();
        down_btn.connect_clicked(move |_| {
            let mut db = ModDatabase::load(&game_c);
            let len = db.mods.len();
            if let Some(pos) = db.mods.iter().position(|m| m.id == mod_id_c)
                && pos + 1 < len
            {
                db.mods.swap(pos, pos + 1);
                db.save(&game_c);
                if let Err(e) = ModManager::rebuild_all(&game_c) {
                    log::error!("Failed to rebuild deployment after reorder: {e}");
                }
                *anchor_c.borrow_mut() = Some(ViewportAnchor {
                    item_key: mod_id_c.clone(),
                    align_ratio: 0.35,
                });
                refresh_library_content_with_search(
                    &container_c,
                    &game_c,
                    Rc::clone(&config_c),
                    &search_c.borrow(),
                    Rc::clone(&search_c),
                    Rc::clone(&selected_c),
                    Rc::clone(&anchor_c),
                    Rc::clone(&drag_scroll_c),
                    false,
                );
            }
        });
    }

    // Drag-and-drop reorder
    let drag_source = gtk4::DragSource::new();
    drag_source.set_actions(gdk::DragAction::MOVE);
    {
        let mod_id_drag = mod_entry.id.clone();
        drag_source.connect_prepare(move |_, _, _| {
            let value = mod_id_drag.to_value();
            Some(gdk::ContentProvider::for_value(&value))
        });
    }
    {
        let row_c = row.clone();
        drag_source.connect_drag_begin(move |src, _| {
            let paintable = gtk4::WidgetPaintable::new(Some(&row_c));
            src.set_icon(Some(&paintable), 0, 0);
        });
    }
    row.add_controller(drag_source);

    let drop_target = gtk4::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
    {
        let game_drop = Rc::clone(game);
        let container_drop = container.clone();
        let config_drop = Rc::clone(&config);
        let search_drop = Rc::clone(&search_state);
        let selected_drop = Rc::clone(&selected_mod_id);
        let anchor_drop = Rc::clone(&pending_viewport_anchor);
        let drag_scroll_drop = Rc::clone(&drag_autoscroll);
        let target_id = mod_entry.id.clone();
        drop_target.connect_drop(move |_, value, _, _| {
            stop_drag_autoscroll(&drag_scroll_drop);
            let Ok(source_id) = value.get::<String>() else {
                return false;
            };
            if source_id == target_id {
                return false;
            }
            let mut db = ModDatabase::load(&game_drop);
            if let (Some(src_pos), Some(tgt_pos)) = (
                db.mods.iter().position(|m| m.id == source_id),
                db.mods.iter().position(|m| m.id == target_id),
            ) {
                let moved = db.mods.remove(src_pos);
                let insert_pos = adjusted_insert_pos(src_pos, tgt_pos);
                db.mods.insert(insert_pos, moved);
                db.save(&game_drop);
                if let Err(e) = ModManager::rebuild_all(&game_drop) {
                    log::error!("Failed to rebuild deployment after drag reorder: {e}");
                }
                *anchor_drop.borrow_mut() = Some(ViewportAnchor {
                    item_key: source_id.clone(),
                    align_ratio: 0.35,
                });
                refresh_library_content_with_search(
                    &container_drop,
                    &game_drop,
                    Rc::clone(&config_drop),
                    &search_drop.borrow(),
                    Rc::clone(&search_drop),
                    Rc::clone(&selected_drop),
                    Rc::clone(&anchor_drop),
                    Rc::clone(&drag_scroll_drop),
                    false,
                );
            }
            true
        });
    }
    row.add_controller(drop_target);

    // ── Uninstall button ─────────────────────────────────────────────────────
    let delete_btn = gtk4::Button::new();
    delete_btn.set_icon_name("user-trash-symbolic");
    delete_btn.set_tooltip_text(Some("Uninstall mod"));
    delete_btn.add_css_class("flat");
    delete_btn.set_valign(gtk4::Align::Center);
    row.add_suffix(&delete_btn);

    let mod_id_del = mod_entry.id.clone();
    let mod_name_del = mod_entry.name.clone();
    let game_del = Rc::clone(game);
    let container_del = container.clone();
    let config_del = Rc::clone(&config);
    let search_del = Rc::clone(&search_state);
    let selected_del = Rc::clone(&selected_mod_id);
    let anchor_for_delete = Rc::clone(&pending_viewport_anchor);
    let drag_scroll_for_delete = Rc::clone(&drag_autoscroll);

    delete_btn.connect_clicked(move |btn| {
        let parent = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());

        let dialog = adw::AlertDialog::builder()
            .heading("Remove Mod?")
            .body(format!(
                "“{}” will be permanently removed from disk.",
                mod_name_del
            ))
            .build();

        dialog.add_response("cancel", "Cancel");
        dialog.add_response("remove", "Remove");
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let mod_id_c = mod_id_del.clone();
        let game_c = Rc::clone(&game_del);
        let container_c = container_del.clone();
        let config_c = Rc::clone(&config_del);
        let search_c = Rc::clone(&search_del);
        let selected_c = Rc::clone(&selected_del);
        let anchor_c = Rc::clone(&anchor_for_delete);
        let drag_scroll_c = Rc::clone(&drag_scroll_for_delete);
        dialog.connect_response(None, move |_, response| {
            if response != "remove" {
                return;
            }
            let db = ModDatabase::load(&game_c);
            if let Some(m) = db.mods.iter().find(|m| m.id == mod_id_c) {
                if let Err(e) = ModManager::uninstall_mod(&game_c, m) {
                    log::error!("Failed to uninstall mod: {e}");
                } else {
                    // Keep downloaded archives on disk, but clear install marker
                    // so Downloads reflects that this mod is no longer installed.
                    let mut cfg = config_c.borrow_mut();
                    let gs = cfg.game_settings_mut(&game_c.id);
                    if let Some(archive_name) = &m.archive_name {
                        gs.installed_archives
                            .retain(|archive| archive != archive_name);
                    } else {
                        let mod_name_lower = m.name.to_lowercase();
                        gs.installed_archives.retain(|archive| {
                            let archive_stem = Path::new(archive)
                                .file_stem()
                                .map(|s| s.to_string_lossy().to_lowercase())
                                .unwrap_or_default();
                            archive_stem != mod_name_lower
                        });
                    }
                    cfg.save();
                }
            }
            if selected_c
                .borrow()
                .as_ref()
                .map(|id| id == &mod_id_c)
                .unwrap_or(false)
            {
                *selected_c.borrow_mut() = None;
            }
            refresh_library_content_with_search(
                &container_c,
                &game_c,
                Rc::clone(&config_c),
                &search_c.borrow(),
                Rc::clone(&search_c),
                Rc::clone(&selected_c),
                Rc::clone(&anchor_c),
                Rc::clone(&drag_scroll_c),
                false,
            );
        });

        dialog.present(parent.as_ref());
    });

    // Left click selects a mod for conflict highlighting
    let left_click = gtk4::GestureClick::new();
    left_click.set_button(1);
    {
        let game_sel = Rc::clone(game);
        let container_sel = container.clone();
        let config_sel = Rc::clone(&config);
        let search_sel = Rc::clone(&search_state);
        let selected_sel = Rc::clone(&selected_mod_id);
        let anchor_sel = Rc::clone(&pending_viewport_anchor);
        let drag_scroll_sel = Rc::clone(&drag_autoscroll);
        let mod_id_sel = mod_entry.id.clone();
        // Use `released` (not `pressed`) so built-in SwitchRow controls process
        // the click first; refreshing immediately on press can swallow toggle
        // interactions and make row controls feel broken.
        left_click.connect_released(move |_, _, _, _| {
            {
                let mut selected = selected_sel.borrow_mut();
                if selected.as_ref() == Some(&mod_id_sel) {
                    // Clicking the same row again clears selection and returns
                    // conflict highlighting to the global blue mode.
                    *selected = None;
                } else {
                    *selected = Some(mod_id_sel.clone());
                }
            }
            // Defer refresh so switch/button default handlers run first; this
            // keeps row toggles and other left-click controls responsive.
            let container_idle = container_sel.clone();
            let game_idle = Rc::clone(&game_sel);
            let config_idle = Rc::clone(&config_sel);
            let search_idle = Rc::clone(&search_sel);
            let selected_idle = Rc::clone(&selected_sel);
            let anchor_idle = Rc::clone(&anchor_sel);
            let drag_scroll_idle = Rc::clone(&drag_scroll_sel);
            gtk4::glib::idle_add_local_once(move || {
                refresh_library_content_with_search(
                    &container_idle,
                    &game_idle,
                    Rc::clone(&config_idle),
                    &search_idle.borrow(),
                    Rc::clone(&search_idle),
                    Rc::clone(&selected_idle),
                    Rc::clone(&anchor_idle),
                    Rc::clone(&drag_scroll_idle),
                    false,
                );
            });
        });
    }
    row.add_controller(left_click);

    // ── Right-click context menu ──────────────────────────────────────────────
    let right_click = gtk4::GestureClick::new();
    right_click.set_button(3);
    {
        let row_c = row.clone();
        let source_path = mod_entry.source_path.clone();
        let nexus_id = mod_entry.nexus_id;
        let game_c = Rc::clone(game);
        let container_rclick = container.clone();
        let config_rclick = Rc::clone(&config);
        let search_rclick = Rc::clone(&search_state);
        let selected_rclick = Rc::clone(&selected_mod_id);
        let anchor_rclick = Rc::clone(&pending_viewport_anchor);
        let drag_scroll_rclick = Rc::clone(&drag_autoscroll);
        let mod_id_rclick = mod_entry.id.clone();
        let conflict_entries = conflict_state
            .map(|state| {
                state
                    .conflict_mods_by_file
                    .iter()
                    .map(|(file, mods)| (file.clone(), mods.iter().cloned().collect::<Vec<_>>()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        right_click.connect_pressed(move |gesture, _, x, y| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);

            let popover = gtk4::Popover::new();
            popover.set_parent(&row_c);
            let rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&rect));
            popover.set_has_arrow(false);

            let menu_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
            menu_box.set_margin_top(4);
            menu_box.set_margin_bottom(4);
            menu_box.set_margin_start(4);
            menu_box.set_margin_end(4);

            let open_dir_item = gtk4::Button::with_label("Open Mod Directory");
            open_dir_item.add_css_class("flat");
            open_dir_item.set_halign(gtk4::Align::Fill);
            open_dir_item.set_hexpand(true);
            menu_box.append(&open_dir_item);

            let open_nexus_item = gtk4::Button::with_label("Visit on Nexus Mods");
            open_nexus_item.add_css_class("flat");
            open_nexus_item.set_halign(gtk4::Align::Fill);
            open_nexus_item.set_hexpand(true);
            open_nexus_item.set_sensitive(nexus_id.is_some());
            menu_box.append(&open_nexus_item);

            let show_conflicts_item = gtk4::Button::with_label("Show Conflicting Files");
            show_conflicts_item.add_css_class("flat");
            show_conflicts_item.set_halign(gtk4::Align::Fill);
            show_conflicts_item.set_hexpand(true);
            show_conflicts_item.set_sensitive(!conflict_entries.is_empty());
            menu_box.append(&show_conflicts_item);

            let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
            menu_box.append(&sep);

            let move_item = gtk4::Button::with_label("Move to Position\u{2026}");
            move_item.add_css_class("flat");
            move_item.set_halign(gtk4::Align::Fill);
            move_item.set_hexpand(true);
            menu_box.append(&move_item);

            let enable_all_item = gtk4::Button::with_label("Enable All");
            enable_all_item.add_css_class("flat");
            enable_all_item.set_halign(gtk4::Align::Fill);
            enable_all_item.set_hexpand(true);
            menu_box.append(&enable_all_item);

            let disable_all_item = gtk4::Button::with_label("Disable All");
            disable_all_item.add_css_class("flat");
            disable_all_item.set_halign(gtk4::Align::Fill);
            disable_all_item.set_hexpand(true);
            menu_box.append(&disable_all_item);

            popover.set_child(Some(&menu_box));

            let popover_dir = popover.clone();
            let source_dir = source_path.clone();
            open_dir_item.connect_clicked(move |_| {
                popover_dir.popdown();
                open_in_file_manager(&source_dir);
            });

            let popover_nexus = popover.clone();
            let game_nexus = Rc::clone(&game_c);
            open_nexus_item.connect_clicked(move |_| {
                popover_nexus.popdown();
                if let Some(id) = nexus_id {
                    let uri = format!(
                        "https://www.nexusmods.com/{}/mods/{}",
                        game_nexus.kind.nexus_game_id(),
                        id
                    );
                    let _ =
                        gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
                }
            });

            let popover_conflicts = popover.clone();
            let row_for_dialog = row_c.clone();
            let conflict_entries_for_menu = conflict_entries.clone();
            show_conflicts_item.connect_clicked(move |_| {
                popover_conflicts.popdown();
                if conflict_entries_for_menu.is_empty() {
                    return;
                }

                let body = conflict_entries_for_menu
                    .iter()
                    .map(|(file, mods)| {
                        if mods.is_empty() {
                            format!("• {file}")
                        } else {
                            format!("• {file}\n  conflicts with: {}", mods.join(", "))
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                let dialog = adw::AlertDialog::builder()
                    .heading("Conflicting Files")
                    .body(&body)
                    .build();
                dialog.add_response("ok", "OK");
                dialog.set_default_response(Some("ok"));
                dialog.set_close_response("ok");
                let parent = row_for_dialog
                    .root()
                    .and_then(|r| r.downcast::<gtk4::Window>().ok());
                dialog.present(parent.as_ref());
            });

            let popover_move = popover.clone();
            let row_move = row_c.clone();
            let game_move = Rc::clone(&game_c);
            let container_move = container_rclick.clone();
            let config_move = Rc::clone(&config_rclick);
            let search_move = Rc::clone(&search_rclick);
            let selected_move = Rc::clone(&selected_rclick);
            let anchor_move = Rc::clone(&anchor_rclick);
            let drag_scroll_move = Rc::clone(&drag_scroll_rclick);
            let mod_id_move = mod_id_rclick.clone();
            move_item.connect_clicked(move |_| {
                popover_move.popdown();
                if let Some(root) = row_move.root()
                    && let Ok(window) = root.downcast::<gtk4::Window>()
                {
                    show_move_to_position_dialog_for_mod(
                        &window,
                        mod_id_move.clone(),
                        Rc::clone(&game_move),
                        container_move.clone(),
                        Rc::clone(&config_move),
                        Rc::clone(&search_move),
                        Rc::clone(&selected_move),
                        Rc::clone(&anchor_move),
                        Rc::clone(&drag_scroll_move),
                    );
                }
            });

            let popover_enable = popover.clone();
            let game_enable = Rc::clone(&game_c);
            let container_enable = container_rclick.clone();
            let config_enable = Rc::clone(&config_rclick);
            let search_enable = Rc::clone(&search_rclick);
            let selected_enable = Rc::clone(&selected_rclick);
            let anchor_enable = Rc::clone(&anchor_rclick);
            let drag_scroll_enable = Rc::clone(&drag_scroll_rclick);
            enable_all_item.connect_clicked(move |_| {
                popover_enable.popdown();
                let mut db = ModDatabase::load(&game_enable);
                for m in db.mods.iter_mut() {
                    m.enabled = true;
                }
                db.save(&game_enable);
                if let Err(e) = ModManager::rebuild_all(&game_enable) {
                    log::error!("Failed to rebuild deployment after enable all: {e}");
                }
                refresh_library_content_with_search(
                    &container_enable,
                    &game_enable,
                    Rc::clone(&config_enable),
                    &search_enable.borrow(),
                    Rc::clone(&search_enable),
                    Rc::clone(&selected_enable),
                    Rc::clone(&anchor_enable),
                    Rc::clone(&drag_scroll_enable),
                    false,
                );
            });

            let popover_disable = popover.clone();
            let game_disable = Rc::clone(&game_c);
            let container_disable = container_rclick.clone();
            let config_disable = Rc::clone(&config_rclick);
            let search_disable = Rc::clone(&search_rclick);
            let selected_disable = Rc::clone(&selected_rclick);
            let anchor_disable = Rc::clone(&anchor_rclick);
            let drag_scroll_disable = Rc::clone(&drag_scroll_rclick);
            disable_all_item.connect_clicked(move |_| {
                popover_disable.popdown();
                let mut db = ModDatabase::load(&game_disable);
                for m in db.mods.iter_mut() {
                    m.enabled = false;
                }
                db.save(&game_disable);
                if let Err(e) = ModManager::rebuild_all(&game_disable) {
                    log::error!("Failed to rebuild deployment after disable all: {e}");
                }
                refresh_library_content_with_search(
                    &container_disable,
                    &game_disable,
                    Rc::clone(&config_disable),
                    &search_disable.borrow(),
                    Rc::clone(&search_disable),
                    Rc::clone(&selected_disable),
                    Rc::clone(&anchor_disable),
                    Rc::clone(&drag_scroll_disable),
                    false,
                );
            });

            popover.popup();
        });
    }
    row.add_controller(right_click);

    row
}

/// Compute target insertion index after removing an item from `src_pos`.
///
/// If `src_pos < target_idx`, removing the source shifts subsequent indices
/// down by one, so the insertion position must be decremented to preserve the
/// intended visual drop target.
fn adjusted_insert_pos(src_pos: usize, target_idx: usize) -> usize {
    if src_pos < target_idx {
        target_idx.saturating_sub(1)
    } else {
        target_idx
    }
}

/// Show a modal dialog that lets the user type a position number for a mod.
/// The valid range is 1 to the total number of installed mods.
fn show_move_to_position_dialog_for_mod(
    parent: &gtk4::Window,
    mod_id: String,
    game: Rc<Game>,
    container: gtk4::Box,
    config: Rc<RefCell<AppConfig>>,
    search_state: Rc<RefCell<String>>,
    selected_mod_id: Rc<RefCell<Option<String>>>,
    pending_viewport_anchor: Rc<RefCell<Option<ViewportAnchor>>>,
    drag_autoscroll: Rc<RefCell<DragAutoScrollState>>,
) {
    let db = ModDatabase::load(&game);
    let total = db.mods.len();
    if total == 0 {
        return;
    }
    let Some(current_pos) = db.mods.iter().position(|m| m.id == mod_id) else {
        return;
    };
    let mod_name = db.mods[current_pos].name.clone();

    let body = format!("Enter the new position for \"{mod_name}\".\nValid range: 1–{total}.",);

    let dialog = adw::AlertDialog::builder()
        .heading("Move to Position")
        .body(&body)
        .build();

    let spin = gtk4::SpinButton::with_range(1.0, total as f64, 1.0);
    spin.set_value((current_pos + 1) as f64);
    spin.set_numeric(true);
    dialog.set_extra_child(Some(&spin));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("move", "Move");
    dialog.set_response_appearance("move", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("move"));
    dialog.set_close_response("cancel");

    dialog.connect_response(None, move |_, response| {
        if response != "move" {
            return;
        }
        let target_pos_1indexed = spin.value() as usize;
        let target_idx = target_pos_1indexed.saturating_sub(1);

        let mut db = ModDatabase::load(&game);
        if let Some(src_pos) = db.mods.iter().position(|m| m.id == mod_id)
            && target_idx < db.mods.len()
        {
            let m = db.mods.remove(src_pos);
            let insert_pos = adjusted_insert_pos(src_pos, target_idx);
            db.mods.insert(insert_pos, m);
            db.save(&game);
            if let Err(e) = ModManager::rebuild_all(&game) {
                log::error!("Failed to rebuild deployment after move: {e}");
            }
            *pending_viewport_anchor.borrow_mut() = Some(ViewportAnchor {
                item_key: mod_id.clone(),
                align_ratio: 0.35,
            });
            refresh_library_content_with_search(
                &container,
                &game,
                Rc::clone(&config),
                &search_state.borrow(),
                Rc::clone(&search_state),
                Rc::clone(&selected_mod_id),
                Rc::clone(&pending_viewport_anchor),
                Rc::clone(&drag_autoscroll),
                false,
            );
        }
    });

    dialog.present(Some(parent));
}

fn compute_conflict_states(
    mods: &[Mod],
    selected_id: Option<&str>,
) -> HashMap<String, ConflictState> {
    let global_states = compute_global_conflict_states(mods);

    if let Some(selected_id) = selected_id {
        let Some(selected_idx) = mods.iter().position(|m| m.id == selected_id) else {
            return global_states;
        };

        let selected_files = collect_mod_target_files(&mods[selected_idx]);
        if selected_files.is_empty() {
            return global_states;
        }

        let mut states: HashMap<String, ConflictState> = HashMap::new();
        for (idx, m) in mods.iter().enumerate() {
            if idx == selected_idx {
                continue;
            }
            let files = collect_mod_target_files(m);
            if files.is_empty() {
                continue;
            }

            let shared: BTreeSet<String> = selected_files.intersection(&files).cloned().collect();
            if shared.is_empty() {
                continue;
            }

            // With selection active: preserve green/red directionality by order.
            if idx > selected_idx {
                states.entry(m.id.clone()).or_default().overwrites = true;
                states
                    .entry(selected_id.to_string())
                    .or_default()
                    .overwritten = true;
            } else {
                states.entry(m.id.clone()).or_default().overwritten = true;
                states
                    .entry(selected_id.to_string())
                    .or_default()
                    .overwrites = true;
            }

            states
                .entry(m.id.clone())
                .or_default()
                .files
                .extend(shared.iter().cloned());
            {
                let entry = states.entry(m.id.clone()).or_default();
                for file in &shared {
                    entry
                        .conflict_mods_by_file
                        .entry(file.clone())
                        .or_default()
                        .insert(mods[selected_idx].name.clone());
                }
            }
            states
                .entry(selected_id.to_string())
                .or_default()
                .files
                .extend(shared.iter().cloned());
            {
                let entry = states.entry(selected_id.to_string()).or_default();
                for file in &shared {
                    entry
                        .conflict_mods_by_file
                        .entry(file.clone())
                        .or_default()
                        .insert(m.name.clone());
                }
            }
        }
        // If selected mod has no conflicts, keep the global blue conflict mode.
        if states.is_empty() {
            global_states
        } else {
            states
        }
    } else {
        global_states
    }
}

fn compute_global_conflict_states(mods: &[Mod]) -> HashMap<String, ConflictState> {
    let mut states: HashMap<String, ConflictState> = HashMap::new();
    let all_files: Vec<BTreeSet<String>> = mods.iter().map(collect_mod_target_files).collect();

    for i in 0..mods.len() {
        if all_files[i].is_empty() {
            continue;
        }
        for j in (i + 1)..mods.len() {
            if all_files[j].is_empty() {
                continue;
            }
            let shared: BTreeSet<String> =
                all_files[i].intersection(&all_files[j]).cloned().collect();
            if shared.is_empty() {
                continue;
            }

            states
                .entry(mods[i].id.clone())
                .or_default()
                .files
                .extend(shared.iter().cloned());
            {
                let entry = states.entry(mods[i].id.clone()).or_default();
                for file in &shared {
                    entry
                        .conflict_mods_by_file
                        .entry(file.clone())
                        .or_default()
                        .insert(mods[j].name.clone());
                }
            }
            states
                .entry(mods[j].id.clone())
                .or_default()
                .files
                .extend(shared.iter().cloned());
            {
                let entry = states.entry(mods[j].id.clone()).or_default();
                for file in &shared {
                    entry
                        .conflict_mods_by_file
                        .entry(file.clone())
                        .or_default()
                        .insert(mods[i].name.clone());
                }
            }
        }
    }

    states
}

fn collect_mod_target_files(mod_entry: &Mod) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    let root = &mod_entry.source_path;
    let data_dir = root.join("Data");

    if data_dir.is_dir() {
        collect_files_recursive(&data_dir, &data_dir, "data", &mut files);

        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.file_name().map(|n| n == "Data").unwrap_or(false) {
                    continue;
                }
                if path.is_dir() {
                    collect_files_recursive(&path, root, "root", &mut files);
                } else if path.is_file()
                    && let Ok(rel) = path.strip_prefix(root)
                {
                    files.insert(normalize_relative_path("root", rel));
                }
            }
        }
    } else {
        collect_files_recursive(root, root, "data", &mut files);
    }

    files
}

fn collect_files_recursive(base: &Path, root: &Path, prefix: &str, files: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, root, prefix, files);
        } else if path.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            files.insert(normalize_relative_path(prefix, rel));
        }
    }
}

fn normalize_relative_path(prefix: &str, rel: &Path) -> String {
    let rel = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
        .to_lowercase();
    format!("{prefix}/{rel}")
}

fn matches_query(value: &str, query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return true;
    }
    value.to_lowercase().contains(&trimmed.to_lowercase())
}

fn open_in_file_manager(path: &Path) {
    let file = gio::File::for_path(path);
    let uri = file.uri();
    let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
}

/// Show a brief in-app toast notification anchored to `widget`.
fn show_toast(widget: &gtk4::Widget, message: &str) {
    // Walk up to the nearest AdwToastOverlay
    let mut ancestor: Option<gtk4::Widget> = Some(widget.clone());
    while let Some(current) = ancestor {
        if let Ok(overlay) = current.clone().downcast::<adw::ToastOverlay>() {
            let toast = adw::Toast::new(message);
            toast.set_timeout(TOAST_TIMEOUT_SECONDS);
            overlay.add_toast(toast);
            return;
        }
        ancestor = current.parent();
    }
    // Fallback: log to stderr
    log::info!("{message}");
}

#[cfg(test)]
mod tests {
    use super::{adjusted_insert_pos, compute_conflict_states, matches_query};
    use crate::core::mods::Mod;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_mod(id: &str, name: &str, path: &str) -> Mod {
        Mod {
            id: id.to_string(),
            name: name.to_string(),
            version: None,
            enabled: false,
            priority: 0,
            nexus_id: None,
            source_path: PathBuf::from(path),
            installed_from_nexus: false,
            archive_name: None,
        }
    }

    #[test]
    fn matches_query_is_case_insensitive() {
        assert!(matches_query("Immersive Armors", "arm"));
        assert!(matches_query("Immersive Armors", "  ARMORS  "));
        assert!(!matches_query("Immersive Armors", "weapons"));
    }

    #[test]
    fn adjusted_insert_pos_accounts_for_source_removal() {
        assert_eq!(adjusted_insert_pos(0, 2), 1);
        assert_eq!(adjusted_insert_pos(3, 1), 1);
    }

    #[test]
    fn compute_conflict_states_returns_empty_without_selection() {
        let mods = vec![sample_mod("a", "A", "/tmp/a")];
        let states = compute_conflict_states(&mods, None);
        assert!(states.is_empty());
    }

    #[test]
    fn compute_conflict_states_detects_shared_files_between_mods() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("linkmm-conflict-test-{unique}"));
        let mod_a = root.join("a");
        let mod_b = root.join("b");
        let mod_c = root.join("c");

        std::fs::create_dir_all(mod_a.join("Data/textures")).unwrap();
        std::fs::create_dir_all(mod_b.join("Data/textures")).unwrap();
        std::fs::create_dir_all(mod_c.join("Data/textures")).unwrap();
        std::fs::write(mod_a.join("Data/textures/sky.dds"), "a").unwrap();
        std::fs::write(mod_b.join("Data/textures/sky.dds"), "b").unwrap();
        std::fs::write(mod_c.join("Data/textures/cloud.dds"), "c").unwrap();

        let mods = vec![
            sample_mod("a", "A", &mod_a.to_string_lossy()),
            sample_mod("b", "B", &mod_b.to_string_lossy()),
            sample_mod("c", "C", &mod_c.to_string_lossy()),
        ];

        let states = compute_conflict_states(&mods, Some("a"));
        let selected = states.get("a").unwrap();
        let conflicting = states.get("b").unwrap();
        assert!(selected.files.contains("data/textures/sky.dds"));
        assert!(conflicting.files.contains("data/textures/sky.dds"));
        assert!(
            selected
                .conflict_mods_by_file
                .get("data/textures/sky.dds")
                .unwrap()
                .contains("B")
        );
        assert!(
            conflicting
                .conflict_mods_by_file
                .get("data/textures/sky.dds")
                .unwrap()
                .contains("A")
        );
        assert!(!states.contains_key("c"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compute_conflict_states_marks_all_conflicting_mods_when_nothing_selected() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("linkmm-conflict-none-test-{unique}"));
        let mod_a = root.join("a");
        let mod_b = root.join("b");

        std::fs::create_dir_all(mod_a.join("Data/textures")).unwrap();
        std::fs::create_dir_all(mod_b.join("Data/textures")).unwrap();
        std::fs::write(mod_a.join("Data/textures/sky.dds"), "a").unwrap();
        std::fs::write(mod_b.join("Data/textures/sky.dds"), "b").unwrap();

        let mods = vec![
            sample_mod("a", "A", &mod_a.to_string_lossy()),
            sample_mod("b", "B", &mod_b.to_string_lossy()),
        ];

        let states = compute_conflict_states(&mods, None);
        let a = states.get("a").unwrap();
        let b = states.get("b").unwrap();
        assert!(a.files.contains("data/textures/sky.dds"));
        assert!(b.files.contains("data/textures/sky.dds"));
        assert!(
            a.conflict_mods_by_file
                .get("data/textures/sky.dds")
                .unwrap()
                .contains("B")
        );
        assert!(
            b.conflict_mods_by_file
                .get("data/textures/sky.dds")
                .unwrap()
                .contains("A")
        );
        assert!(!a.overwrites && !a.overwritten);
        assert!(!b.overwrites && !b.overwritten);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compute_conflict_states_classifies_overwrite_direction_with_selection() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("linkmm-conflict-dir-test-{unique}"));
        let mod_a = root.join("a");
        let mod_b = root.join("b");

        std::fs::create_dir_all(mod_a.join("Data/textures")).unwrap();
        std::fs::create_dir_all(mod_b.join("Data/textures")).unwrap();
        std::fs::write(mod_a.join("Data/textures/sky.dds"), "a").unwrap();
        std::fs::write(mod_b.join("Data/textures/sky.dds"), "b").unwrap();

        let mods = vec![
            sample_mod("a", "A", &mod_a.to_string_lossy()),
            sample_mod("b", "B", &mod_b.to_string_lossy()),
        ];

        let states = compute_conflict_states(&mods, Some("a"));
        let selected = states.get("a").unwrap();
        let other = states.get("b").unwrap();
        assert!(selected.overwritten);
        assert!(!selected.overwrites);
        assert!(other.overwrites);
        assert!(!other.overwritten);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compute_conflict_states_keeps_global_blue_conflicts_when_selected_mod_has_none() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("linkmm-conflict-fallback-test-{unique}"));
        let mod_a = root.join("a");
        let mod_b = root.join("b");
        let mod_c = root.join("c");

        std::fs::create_dir_all(mod_a.join("Data/textures")).unwrap();
        std::fs::create_dir_all(mod_b.join("Data/textures")).unwrap();
        std::fs::create_dir_all(mod_c.join("Data/meshes")).unwrap();
        std::fs::write(mod_a.join("Data/textures/sky.dds"), "a").unwrap();
        std::fs::write(mod_b.join("Data/textures/sky.dds"), "b").unwrap();
        std::fs::write(mod_c.join("Data/meshes/rock.nif"), "c").unwrap();

        let mods = vec![
            sample_mod("a", "A", &mod_a.to_string_lossy()),
            sample_mod("b", "B", &mod_b.to_string_lossy()),
            sample_mod("c", "C", &mod_c.to_string_lossy()),
        ];

        // Select C (no conflicts) -> A/B must still remain in blue-mode data.
        let states = compute_conflict_states(&mods, Some("c"));
        let a = states.get("a").unwrap();
        let b = states.get("b").unwrap();
        assert!(a.files.contains("data/textures/sky.dds"));
        assert!(b.files.contains("data/textures/sky.dds"));
        assert!(!a.overwrites && !a.overwritten);
        assert!(!b.overwrites && !b.overwritten);
        assert!(!states.contains_key("c"));

        std::fs::remove_dir_all(root).unwrap();
    }
}
