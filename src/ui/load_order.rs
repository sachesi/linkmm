use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::games::Game;
use crate::core::mods::{ModDatabase, PluginFile};
use crate::ui::ordering;

const CLAMP_MARGIN: f64 = 12.0;
const SCROLL_EDGE: f64 = 60.0;
const SCROLL_SPEED: f64 = 12.0;

/// Build the Load Order page for `game`.
pub fn build_load_order_page(game: Option<&Game>) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let title_widget = adw::WindowTitle::new("Load Order", "");
    header.set_title_widget(Some(&title_widget));

    let search_entry = gtk4::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Search plugins"));
    search_entry.set_width_chars(24);
    search_entry.set_sensitive(game.is_some());
    header.pack_start(&search_entry);

    let sort_btn = gtk4::Button::new();
    sort_btn.set_icon_name("view-sort-ascending-symbolic");
    sort_btn.set_sensitive(game.is_some());
    header.pack_end(&sort_btn);

    toolbar_view.add_top_bar(&header);

    let content_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_container.set_vexpand(true);

    match game {
        None => {
            sort_btn.set_tooltip_text(Some("No game selected"));
            let status = adw::StatusPage::builder()
                .title("No Game Selected")
                .description("Select a game from the sidebar to manage its load order.")
                .icon_name("applications-games-symbolic")
                .build();
            status.set_vexpand(true);
            content_container.append(&status);
        }
        Some(g) if !g.kind.has_plugins_txt() => {
            sort_btn.set_sensitive(false);
            sort_btn.set_tooltip_text(Some("Not supported for this game"));
            let status = adw::StatusPage::builder()
                .title("Load Order Not Supported")
                .description(
                    "Morrowind stores its plugin list in Morrowind.ini rather than plugins.txt.\n\
                     Plugin order management is not yet supported for this game.",
                )
                .icon_name("dialog-information-symbolic")
                .build();
            status.set_vexpand(true);
            content_container.append(&status);
        }
        Some(g) => {
            sort_btn.set_tooltip_text(Some("Sort plugins using LOOT load order rules"));
            let game_rc = Rc::new(g.clone());
            let search_query = Rc::new(RefCell::new(String::new()));

            let reorder_hint = gtk4::Label::new(Some("Clear search to reorder."));
            reorder_hint.add_css_class("dim-label");
            reorder_hint.set_margin_top(8);
            reorder_hint.set_margin_bottom(4);
            reorder_hint.set_margin_start(16);
            reorder_hint.set_margin_end(16);
            reorder_hint.set_halign(gtk4::Align::Start);
            reorder_hint.set_visible(false);
            content_container.append(&reorder_hint);

            let stack = gtk4::Stack::new();
            stack.set_vexpand(true);

            let list_box = gtk4::ListBox::new();
            list_box.add_css_class("boxed-list");
            list_box.set_selection_mode(gtk4::SelectionMode::None);

            let clamp = adw::Clamp::new();
            clamp.set_maximum_size(900);
            clamp.set_margin_top(12);
            clamp.set_margin_bottom(12);
            clamp.set_margin_start(12);
            clamp.set_margin_end(12);
            clamp.set_child(Some(&list_box));

            let scrolled = gtk4::ScrolledWindow::new();
            scrolled.set_vexpand(true);
            scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
            scrolled.set_child(Some(&clamp));

            let status_page = adw::StatusPage::new();
            status_page.set_vexpand(true);

            stack.add_named(&scrolled, Some("list"));
            stack.add_named(&status_page, Some("status"));
            stack.set_visible_child_name("list");
            content_container.append(&stack);

            {
                let game_c = Rc::clone(&game_rc);
                let list_c = list_box.clone();
                let scrolled_c = scrolled.clone();
                let status_c = status_page.clone();
                let stack_c = stack.clone();
                let search_c = Rc::clone(&search_query);
                let hint_c = reorder_hint.clone();
                search_entry.connect_search_changed(move |entry| {
                    *search_c.borrow_mut() = entry.text().to_string();
                    refresh_load_order_content(
                        &list_c,
                        &scrolled_c,
                        &status_c,
                        &stack_c,
                        &hint_c,
                        &game_c,
                        &search_c.borrow(),
                    );
                });
            }

            {
                let game_c = Rc::clone(&game_rc);
                let list_c = list_box.clone();
                let scrolled_c = scrolled.clone();
                let status_c = status_page.clone();
                let stack_c = stack.clone();
                let search_c = Rc::clone(&search_query);
                let hint_c = reorder_hint.clone();
                sort_btn.connect_clicked(move |_| {
                    let mut db = ModDatabase::load(&game_c);
                    db.sync_from_plugins_txt(&game_c);
                    db.sort_plugins_by_type(&game_c);
                    db.save(&game_c);
                    if let Err(e) = db.write_plugins_txt(&game_c) {
                        log::warn!("Failed to write plugins.txt after sort: {e}");
                    }
                    refresh_load_order_content(
                        &list_c,
                        &scrolled_c,
                        &status_c,
                        &stack_c,
                        &hint_c,
                        &game_c,
                        &search_c.borrow(),
                    );
                });
            }

            refresh_load_order_content(
                &list_box,
                &scrolled,
                &status_page,
                &stack,
                &reorder_hint,
                &game_rc,
                &search_query.borrow(),
            );
        }
    };

    toolbar_view.set_content(Some(&content_container));
    toolbar_view.upcast()
}

// ── Per-row data for in-place DnD ─────────────────────────────────────────────

struct DndPluginRow {
    plugin_name: String,
    row: adw::ActionRow,
    index_label: gtk4::Label,
    up_btn: gtk4::Button,
    down_btn: gtk4::Button,
}

struct PluginRowResult {
    row: adw::ActionRow,
    index_label: gtk4::Label,
    up_btn: gtk4::Button,
    down_btn: gtk4::Button,
    is_vanilla: bool,
}

#[allow(clippy::too_many_arguments)]
fn refresh_load_order_content(
    list_box: &gtk4::ListBox,
    scrolled: &gtk4::ScrolledWindow,
    status_page: &adw::StatusPage,
    stack: &gtk4::Stack,
    reorder_hint: &gtk4::Label,
    game: &Rc<Game>,
    search_query: &str,
) {
    let is_filtered = !search_query.trim().is_empty();
    reorder_hint.set_visible(is_filtered);

    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let mut db = ModDatabase::load(game);
    db.sync_from_plugins_txt(game);

    let ordered_plugins = db.get_ordered_plugins(game);
    if ordered_plugins.is_empty() {
        status_page.set_title("No Plugins Found");
        status_page.set_description(Some(
            "No .esm / .esl / .esp files were found in the game's Data directory.\nInstall and deploy mods first, or check that the game path is correct.",
        ));
        status_page.set_icon_name(Some("format-justify-left-symbolic"));
        stack.set_visible_child_name("status");
        return;
    }

    let filtered: Vec<PluginFile> = ordered_plugins
        .iter()
        .filter(|p| {
            matches_query(&p.name, search_query) || matches_query(p.kind.label(), search_query)
        })
        .cloned()
        .collect();

    if filtered.is_empty() {
        status_page.set_title("No Matching Plugins");
        status_page.set_description(Some("No plugins match your search."));
        status_page.set_icon_name(Some("system-search-symbolic"));
        stack.set_visible_child_name("status");
        return;
    }

    let pinned_prefix_len = ordered_plugins.iter().take_while(|p| p.is_vanilla).count();
    let allow_reorder = !is_filtered;

    let mut dnd_rows: Vec<DndPluginRow> = Vec::new();

    for plugin in &filtered {
        let Some(full_index) = ordered_plugins.iter().position(|p| p.name == plugin.name) else {
            continue;
        };
        let result = build_plugin_row(
            plugin,
            full_index,
            ordered_plugins.len(),
            pinned_prefix_len,
            allow_reorder,
            game,
            list_box,
            status_page,
            stack,
            reorder_hint,
            search_query,
        );
        list_box.append(&result.row);
        if !result.is_vanilla {
            dnd_rows.push(DndPluginRow {
                plugin_name: plugin.name.clone(),
                row: result.row,
                index_label: result.index_label,
                up_btn: result.up_btn,
                down_btn: result.down_btn,
            });
        }
    }

    stack.set_visible_child_name("list");

    if allow_reorder && !dnd_rows.is_empty() {
        let dnd_rows_rc = Rc::new(RefCell::new(dnd_rows));
        setup_load_order_dnd(
            list_box,
            scrolled,
            dnd_rows_rc,
            Rc::clone(game),
            pinned_prefix_len,
            ordered_plugins.len(),
        );
    }
}

// ── Drag & drop for load order ────────────────────────────────────────────────

fn clear_lo_dnd_indicator(indicator: &Rc<RefCell<Option<(gtk4::Widget, bool)>>>) {
    if let Some((w, is_before)) = indicator.borrow_mut().take() {
        if is_before {
            w.remove_css_class("dnd-drop-before");
        } else {
            w.remove_css_class("dnd-drop-after");
        }
    }
}

fn setup_load_order_dnd(
    list_box: &gtk4::ListBox,
    scrolled: &gtk4::ScrolledWindow,
    dnd_rows: Rc<RefCell<Vec<DndPluginRow>>>,
    game: Rc<Game>,
    pinned_prefix_len: usize,
    total_plugins: usize,
) {
    let indicator: Rc<RefCell<Option<(gtk4::Widget, bool)>>> = Rc::new(RefCell::new(None));
    let scroll_dir: Rc<Cell<i8>> = Rc::new(Cell::new(0));
    let autoscroll_id: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

    let drop_target =
        gtk4::DropTarget::new(glib::Type::STRING, gtk4::gdk::DragAction::MOVE);

    // ── motion ────────────────────────────────────────────────────────────────
    {
        let list_box_m = list_box.clone();
        let scrolled_m = scrolled.clone();
        let scrolled_a = scrolled.clone();
        let dnd_rows_m = Rc::clone(&dnd_rows);
        let indicator_m = Rc::clone(&indicator);
        let scroll_dir_m = Rc::clone(&scroll_dir);
        let autoscroll_m = Rc::clone(&autoscroll_id);

        drop_target.connect_motion(move |_, _x, y| {
            let h = scrolled_m.height() as f64;
            let new_dir: i8 = if y < SCROLL_EDGE {
                -1
            } else if y > h - SCROLL_EDGE {
                1
            } else {
                0
            };
            let old_dir = scroll_dir_m.get();
            scroll_dir_m.set(new_dir);
            if old_dir == 0 && new_dir != 0 {
                let scrolled_t = scrolled_a.clone();
                let dir_t = Rc::clone(&scroll_dir_m);
                let id = glib::timeout_add_local(Duration::from_millis(16), move || {
                    let d = dir_t.get() as f64 * SCROLL_SPEED;
                    let adj = scrolled_t.vadjustment();
                    let max = (adj.upper() - adj.page_size()).max(0.0);
                    adj.set_value((adj.value() + d).clamp(0.0, max));
                    glib::ControlFlow::Continue
                });
                *autoscroll_m.borrow_mut() = Some(id);
            } else if old_dir != 0 && new_dir == 0 {
                if let Some(id) = autoscroll_m.borrow_mut().take() {
                    id.remove();
                }
            }

            let adj = scrolled_m.vadjustment();
            let listbox_y = (y + adj.value() - CLAMP_MARGIN) as i32;

            let rows = dnd_rows_m.borrow();
            let (target_widget, is_before) = match list_box_m.row_at_y(listbox_y) {
                Some(ref trow) => {
                    let before = trow
                        .compute_bounds(&list_box_m)
                        .map_or(true, |b| listbox_y < (b.y() + b.height() / 2.0) as i32);
                    (trow.clone().upcast::<gtk4::Widget>(), before)
                }
                None => match rows.last() {
                    Some(last) => (last.row.clone().upcast::<gtk4::Widget>(), false),
                    None => return gtk4::gdk::DragAction::MOVE,
                },
            };
            drop(rows);

            {
                let mut ind = indicator_m.borrow_mut();
                if let Some((prev_w, prev_before)) = ind.as_ref() {
                    if *prev_before {
                        prev_w.remove_css_class("dnd-drop-before");
                    } else {
                        prev_w.remove_css_class("dnd-drop-after");
                    }
                }
                if is_before {
                    target_widget.add_css_class("dnd-drop-before");
                } else {
                    target_widget.add_css_class("dnd-drop-after");
                }
                *ind = Some((target_widget, is_before));
            }

            gtk4::gdk::DragAction::MOVE
        });
    }

    // ── leave ─────────────────────────────────────────────────────────────────
    {
        let indicator_l = Rc::clone(&indicator);
        let autoscroll_l = Rc::clone(&autoscroll_id);
        let scroll_dir_l = Rc::clone(&scroll_dir);
        drop_target.connect_leave(move |_| {
            clear_lo_dnd_indicator(&indicator_l);
            if let Some(id) = autoscroll_l.borrow_mut().take() {
                id.remove();
            }
            scroll_dir_l.set(0);
        });
    }

    // ── drop ──────────────────────────────────────────────────────────────────
    {
        let list_box_d = list_box.clone();
        let scrolled_d = scrolled.clone();
        let dnd_rows_d = Rc::clone(&dnd_rows);
        let game_d = Rc::clone(&game);
        let indicator_d = Rc::clone(&indicator);
        let autoscroll_d = Rc::clone(&autoscroll_id);
        let scroll_dir_d = Rc::clone(&scroll_dir);

        drop_target.connect_drop(move |_, value, _x, y| {
            if let Some(id) = autoscroll_d.borrow_mut().take() {
                id.remove();
            }
            scroll_dir_d.set(0);
            clear_lo_dnd_indicator(&indicator_d);

            let Ok(source_name) = value.get::<String>() else {
                return false;
            };

            let adj = scrolled_d.vadjustment();
            let listbox_y = (y + adj.value() - CLAMP_MARGIN) as i32;

            // Determine source index in the dnd_rows (draggable / non-vanilla only)
            let rows = dnd_rows_d.borrow();
            let Some(dnd_source_idx) = rows.iter().position(|r| r.plugin_name == source_name)
            else {
                return false;
            };

            // Find target row under cursor
            let dnd_target_idx = match list_box_d.row_at_y(listbox_y) {
                Some(ref trow) => {
                    let tptr = trow.upcast_ref::<glib::Object>().as_ptr() as usize;
                    // Skip vanilla rows which aren't in dnd_rows; find in dnd_rows
                    let row_pos = rows
                        .iter()
                        .position(|r| {
                            r.row.upcast_ref::<glib::Object>().as_ptr() as usize == tptr
                        });
                    match row_pos {
                        Some(pos) => {
                            let before = trow
                                .compute_bounds(&list_box_d)
                                .map_or(true, |b| {
                                    listbox_y < (b.y() + b.height() / 2.0) as i32
                                });
                            if before { pos } else { pos + 1 }
                        }
                        // Dropped on a vanilla row or unknown row → reject
                        None => {
                            // Check if it's a vanilla row (in the list but not in dnd_rows)
                            // In that case we want to insert after all vanilla rows = index 0 in dnd_rows
                            // For simplicity, reject drops on vanilla rows entirely.
                            return true;
                        }
                    }
                }
                None => rows.len(), // below all rows → append at end
            };
            drop(rows);

            if dnd_target_idx == dnd_source_idx || dnd_target_idx == dnd_source_idx + 1 {
                return true;
            }

            let insert_idx =
                ordering::insertion_index_after_removal(dnd_source_idx, dnd_target_idx);

            let saved_scroll = adj.value();

            // Move widget in ListBox
            let mut rows = dnd_rows_d.borrow_mut();
            let src_widget = rows[dnd_source_idx].row.clone().upcast::<gtk4::Widget>();
            list_box_d.remove(&src_widget);
            list_box_d.insert(&src_widget, (pinned_prefix_len + insert_idx) as i32);

            // Reorder dnd_rows bookkeeping
            let row_data = rows.remove(dnd_source_idx);
            rows.insert(insert_idx, row_data);

            // Update index labels and button sensitivity for all movable rows
            for (i, r) in rows.iter().enumerate() {
                let full_idx = pinned_prefix_len + i;
                r.index_label.set_text(&format!("{}", full_idx + 1));
                r.up_btn.set_sensitive(full_idx > pinned_prefix_len);
                r.down_btn.set_sensitive(full_idx + 1 < total_plugins);
            }

            // Build the new full plugin order and persist
            let new_order: Vec<String> = rows.iter().map(|r| r.plugin_name.clone()).collect();
            drop(rows);

            glib::idle_add_local_once(move || {
                adj.set_value(saved_scroll);
                let adj2 = adj.clone();
                glib::idle_add_local_once(move || { adj2.set_value(saved_scroll); });
            });

            let game_c = Rc::clone(&game_d);
            glib::idle_add_local_once(move || {
                let mut db = ModDatabase::load(&game_c);
                db.sync_from_plugins_txt(&game_c);
                let ordered = db.get_ordered_plugins(&game_c);
                // Build the reordered list: vanilla first, then new_order
                let mut reordered: Vec<PluginFile> =
                    ordered.iter().filter(|p| p.is_vanilla).cloned().collect();
                for name in &new_order {
                    if let Some(p) = ordered.iter().find(|p| &p.name == name) {
                        reordered.push(p.clone());
                    }
                }
                // Append any plugins not in new_order (edge case)
                for p in &ordered {
                    if !p.is_vanilla && !reordered.iter().any(|r| r.name == p.name) {
                        reordered.push(p.clone());
                    }
                }
                db.set_plugin_order(&reordered);
                db.save(&game_c);
                if let Err(e) = db.write_plugins_txt(&game_c) {
                    log::warn!("Failed to write plugins.txt after DnD reorder: {e}");
                }
            });

            true
        });
    }

    scrolled.add_controller(drop_target);
}

// ── Plugin row builder ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_plugin_row(
    plugin: &PluginFile,
    full_index: usize,
    total: usize,
    pinned_prefix_len: usize,
    allow_reorder: bool,
    game: &Rc<Game>,
    list_box: &gtk4::ListBox,
    status_page: &adw::StatusPage,
    stack: &gtk4::Stack,
    reorder_hint: &gtk4::Label,
    search_query: &str,
) -> PluginRowResult {
    let subtitle = if plugin.is_vanilla {
        format!("{} · Vanilla master (pinned)", plugin.kind.label())
    } else {
        plugin.kind.label().to_string()
    };

    let row = adw::ActionRow::builder()
        .title(&plugin.name)
        .subtitle(&subtitle)
        .build();

    let index_label = gtk4::Label::new(Some(&format!("{}", full_index + 1)));
    index_label.add_css_class("dim-label");
    index_label.add_css_class("numeric");
    index_label.set_width_chars(3);

    // ── Drag handle (non-vanilla only) ────────────────────────────────────────
    if !plugin.is_vanilla {
        let handle = gtk4::Button::new();
        handle.set_icon_name("list-drag-handle-symbolic");
        handle.add_css_class("flat");
        handle.add_css_class("drag-handle");
        handle.set_valign(gtk4::Align::Center);
        handle.set_sensitive(allow_reorder);
        handle.set_tooltip_text(Some(if allow_reorder {
            "Drag to reorder"
        } else {
            "Clear search to reorder"
        }));
        row.add_prefix(&handle);

        if allow_reorder {
            let drag_source = gtk4::DragSource::new();
            drag_source.set_actions(gtk4::gdk::DragAction::MOVE);
            let content =
                gtk4::gdk::ContentProvider::for_value(&plugin.name.to_value());
            drag_source.set_content(Some(&content));

            let row_begin = row.clone();
            drag_source.connect_drag_begin(move |src, _| {
                let paintable = gtk4::WidgetPaintable::new(Some(&row_begin));
                src.set_icon(Some(&paintable), 16, 28);
                row_begin.add_css_class("dnd-source");
            });

            let row_end = row.clone();
            drag_source.connect_drag_end(move |_, _, _| {
                row_end.remove_css_class("dnd-source");
            });

            handle.add_controller(drag_source);
        }
    }

    row.add_prefix(&index_label);

    let badge = gtk4::Label::new(Some(plugin.kind.label()));
    badge.add_css_class("caption");
    badge.add_css_class("dim-label");
    badge.set_valign(gtk4::Align::Center);
    row.add_suffix(&badge);

    if plugin.is_vanilla {
        let lock = gtk4::Image::from_icon_name("changes-prevent-symbolic");
        lock.set_tooltip_text(Some("Vanilla master – cannot be moved or disabled"));
        lock.add_css_class("dim-label");
        row.add_suffix(&lock);
        return PluginRowResult {
            row,
            index_label,
            up_btn: gtk4::Button::new(), // placeholder, not used
            down_btn: gtk4::Button::new(),
            is_vanilla: true,
        };
    }

    let enabled_btn = gtk4::CheckButton::new();
    enabled_btn.set_active(plugin.enabled);
    enabled_btn.set_tooltip_text(Some(if plugin.enabled {
        "Disable plugin"
    } else {
        "Enable plugin"
    }));
    enabled_btn.set_valign(gtk4::Align::Center);
    {
        let game_c = Rc::clone(game);
        let list_c = list_box.clone();
        let status_c = status_page.clone();
        let stack_c = stack.clone();
        let hint_c = reorder_hint.clone();
        let plugin_name = plugin.name.clone();
        let search_q = search_query.to_string();
        enabled_btn.connect_toggled(move |btn| {
            let enabled = btn.is_active();
            let mut db = ModDatabase::load(&game_c);
            db.sync_from_plugins_txt(&game_c);
            if enabled {
                db.enable_plugin(&plugin_name);
            } else {
                db.disable_plugin(&plugin_name);
            }
            db.save(&game_c);
            let _ = db.write_plugins_txt(&game_c);
            // Refresh by re-fetching the scrolled window from the list's actual parent
            if let Some(clamp) = list_c.parent()
                && let Some(sw) = clamp.parent()
                && let Ok(scrolled) = sw.downcast::<gtk4::ScrolledWindow>()
            {
                refresh_load_order_content(
                    &list_c,
                    &scrolled,
                    &status_c,
                    &stack_c,
                    &hint_c,
                    &game_c,
                    &search_q,
                );
            }
        });
    }
    row.add_suffix(&enabled_btn);

    let up_btn = gtk4::Button::new();
    up_btn.set_icon_name("go-up-symbolic");
    up_btn.set_valign(gtk4::Align::Center);
    up_btn.add_css_class("flat");
    up_btn.set_tooltip_text(Some(if allow_reorder {
        "Move up"
    } else {
        "Clear search to reorder"
    }));
    up_btn.set_sensitive(allow_reorder && full_index > pinned_prefix_len);

    let down_btn = gtk4::Button::new();
    down_btn.set_icon_name("go-down-symbolic");
    down_btn.set_valign(gtk4::Align::Center);
    down_btn.add_css_class("flat");
    down_btn.set_tooltip_text(Some(if allow_reorder {
        "Move down"
    } else {
        "Clear search to reorder"
    }));
    down_btn.set_sensitive(allow_reorder && full_index + 1 < total);

    row.add_suffix(&up_btn);
    row.add_suffix(&down_btn);

    {
        let game_c = Rc::clone(game);
        let list_c = list_box.clone();
        let status_c = status_page.clone();
        let stack_c = stack.clone();
        let hint_c = reorder_hint.clone();
        let plugin_name = plugin.name.clone();
        let search_q = search_query.to_string();
        up_btn.connect_clicked(move |_| {
            if let Some(scrolled) = scrolled_from_list(&list_c) {
                let mut db = ModDatabase::load(&game_c);
                db.sync_from_plugins_txt(&game_c);
                let ordered = db.get_ordered_plugins(&game_c);
                if let Ok(updated) = ordering::move_up_by_id(
                    &ordered,
                    &plugin_name,
                    pinned_prefix_len,
                    |p| &p.name,
                ) {
                    db.set_plugin_order(&updated);
                    db.save(&game_c);
                    let _ = db.write_plugins_txt(&game_c);
                }
                refresh_load_order_content(
                    &list_c,
                    &scrolled,
                    &status_c,
                    &stack_c,
                    &hint_c,
                    &game_c,
                    &search_q,
                );
            }
        });
    }

    {
        let game_c = Rc::clone(game);
        let list_c = list_box.clone();
        let status_c = status_page.clone();
        let stack_c = stack.clone();
        let hint_c = reorder_hint.clone();
        let plugin_name = plugin.name.clone();
        let search_q = search_query.to_string();
        down_btn.connect_clicked(move |_| {
            if let Some(scrolled) = scrolled_from_list(&list_c) {
                let mut db = ModDatabase::load(&game_c);
                db.sync_from_plugins_txt(&game_c);
                let ordered = db.get_ordered_plugins(&game_c);
                if let Ok(updated) = ordering::move_down_by_id(
                    &ordered,
                    &plugin_name,
                    pinned_prefix_len,
                    |p| &p.name,
                ) {
                    db.set_plugin_order(&updated);
                    db.save(&game_c);
                    let _ = db.write_plugins_txt(&game_c);
                }
                refresh_load_order_content(
                    &list_c,
                    &scrolled,
                    &status_c,
                    &stack_c,
                    &hint_c,
                    &game_c,
                    &search_q,
                );
            }
        });
    }

    let right_click = gtk4::GestureClick::new();
    right_click.set_button(3);
    {
        let row_c = row.clone();
        let game_rclick = Rc::clone(game);
        let list_rclick = list_box.clone();
        let status_rclick = status_page.clone();
        let stack_rclick = stack.clone();
        let hint_rclick = reorder_hint.clone();
        let plugin_name_rclick = plugin.name.clone();
        let search_q = search_query.to_string();
        right_click.connect_pressed(move |gesture, _, x, y| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);

            let popover = gtk4::Popover::new();
            popover.set_parent(&row_c);
            let rect = gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&rect));
            popover.set_has_arrow(false);

            let menu_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
            menu_box.set_margin_top(4);
            menu_box.set_margin_bottom(4);
            menu_box.set_margin_start(4);
            menu_box.set_margin_end(4);

            let move_item = gtk4::Button::with_label("Move to Position…");
            move_item.add_css_class("flat");
            move_item.set_halign(gtk4::Align::Fill);
            move_item.set_hexpand(true);
            move_item.set_sensitive(allow_reorder);
            move_item.set_tooltip_text(Some(if allow_reorder {
                "Move to a specific load-order position"
            } else {
                "Clear search to reorder"
            }));
            menu_box.append(&move_item);

            let sep = gtk4::Separator::new(gtk4::Orientation::Horizontal);
            menu_box.append(&sep);

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

            let popover_c = popover.clone();
            let row_btn = row_c.clone();
            let game_btn = Rc::clone(&game_rclick);
            let list_btn = list_rclick.clone();
            let status_btn = status_rclick.clone();
            let stack_btn = stack_rclick.clone();
            let hint_btn = hint_rclick.clone();
            let plugin_name_btn = plugin_name_rclick.clone();
            let search_btn = search_q.clone();
            move_item.connect_clicked(move |_| {
                popover_c.popdown();
                if let Some(root) = row_btn.root()
                    && let Ok(window) = root.downcast::<gtk4::Window>()
                {
                    show_move_to_position_dialog(
                        &window,
                        plugin_name_btn.clone(),
                        pinned_prefix_len,
                        Rc::clone(&game_btn),
                        list_btn.clone(),
                        status_btn.clone(),
                        stack_btn.clone(),
                        hint_btn.clone(),
                        search_btn.clone(),
                    );
                }
            });

            let popover_enable = popover.clone();
            let game_enable = Rc::clone(&game_rclick);
            let list_enable = list_rclick.clone();
            let status_enable = status_rclick.clone();
            let stack_enable = stack_rclick.clone();
            let hint_enable = hint_rclick.clone();
            let search_enable = search_q.clone();
            enable_all_item.connect_clicked(move |_| {
                popover_enable.popdown();
                if let Some(scrolled) = scrolled_from_list(&list_enable) {
                    let mut db = ModDatabase::load(&game_enable);
                    db.plugin_disabled.clear();
                    db.save(&game_enable);
                    let _ = db.write_plugins_txt(&game_enable);
                    refresh_load_order_content(
                        &list_enable,
                        &scrolled,
                        &status_enable,
                        &stack_enable,
                        &hint_enable,
                        &game_enable,
                        &search_enable,
                    );
                }
            });

            let popover_disable = popover.clone();
            let game_disable = Rc::clone(&game_rclick);
            let list_disable = list_rclick.clone();
            let status_disable = status_rclick.clone();
            let stack_disable = stack_rclick.clone();
            let hint_disable = hint_rclick.clone();
            let search_disable = search_q.clone();
            disable_all_item.connect_clicked(move |_| {
                popover_disable.popdown();
                if let Some(scrolled) = scrolled_from_list(&list_disable) {
                    let mut db = ModDatabase::load(&game_disable);
                    let ordered = db.get_ordered_plugins(&game_disable);
                    for p in &ordered {
                        if !p.is_vanilla {
                            db.disable_plugin(&p.name);
                        }
                    }
                    db.save(&game_disable);
                    let _ = db.write_plugins_txt(&game_disable);
                    refresh_load_order_content(
                        &list_disable,
                        &scrolled,
                        &status_disable,
                        &stack_disable,
                        &hint_disable,
                        &game_disable,
                        &search_disable,
                    );
                }
            });

            popover.popup();
        });
    }
    row.add_controller(right_click);

    PluginRowResult {
        row,
        index_label,
        up_btn,
        down_btn,
        is_vanilla: false,
    }
}

/// Walk up from a `ListBox` to find its ancestor `ScrolledWindow`.
fn scrolled_from_list(list_box: &gtk4::ListBox) -> Option<gtk4::ScrolledWindow> {
    list_box
        .parent() // Clamp
        .and_then(|p| p.parent()) // ScrolledWindow
        .and_then(|p| p.downcast::<gtk4::ScrolledWindow>().ok())
}

#[allow(clippy::too_many_arguments)]
fn show_move_to_position_dialog(
    parent: &gtk4::Window,
    plugin_name: String,
    pinned_prefix_len: usize,
    game: Rc<Game>,
    list_box: gtk4::ListBox,
    status_page: adw::StatusPage,
    stack: gtk4::Stack,
    reorder_hint: gtk4::Label,
    search_query: String,
) {
    let mut db = ModDatabase::load(&game);
    db.sync_from_plugins_txt(&game);
    let ordered = db.get_ordered_plugins(&game);
    let total = ordered.len();
    let Some(current_idx) = ordered.iter().position(|p| p.name == plugin_name) else {
        return;
    };

    let min_pos = pinned_prefix_len + 1;
    let body = if pinned_prefix_len > 0 {
        format!(
            "Enter the new load order position for \"{plugin_name}\".\nValid range: {min_pos}\u{2013}{total} (positions 1\u{2013}{pinned_prefix_len} are vanilla masters).",
        )
    } else {
        format!("Enter the new load order position for \"{plugin_name}\".\nValid range: 1\u{2013}{total}.")
    };

    ordering::show_position_dialog(
        parent,
        "Move to Position",
        &body,
        min_pos,
        total,
        current_idx + 1,
        move |target_idx| {
            if let Some(scrolled) = scrolled_from_list(&list_box) {
                let mut db = ModDatabase::load(&game);
                db.sync_from_plugins_txt(&game);
                let ordered = db.get_ordered_plugins(&game);
                if let Ok(updated) = ordering::move_to_absolute_position_by_id(
                    &ordered,
                    &plugin_name,
                    target_idx,
                    pinned_prefix_len,
                    |p| &p.name,
                ) {
                    db.set_plugin_order(&updated);
                    db.save(&game);
                    let _ = db.write_plugins_txt(&game);
                }
                refresh_load_order_content(
                    &list_box,
                    &scrolled,
                    &status_page,
                    &stack,
                    &reorder_hint,
                    &game,
                    &search_query,
                );
            }
        },
    );
}

fn matches_query(value: &str, query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return true;
    }
    value.to_lowercase().contains(&trimmed.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::matches_query;

    #[test]
    fn matches_query_is_case_insensitive() {
        assert!(matches_query("MyPatch.esp", "patch"));
        assert!(matches_query("MyPatch.esp", "  ESP  "));
        assert!(!matches_query("MyPatch.esp", "armor"));
    }
}
