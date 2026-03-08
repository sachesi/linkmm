use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::games::Game;
use crate::core::mods::{ModDatabase, PluginFile};

/// Build the Load Order page for `game`.
///
/// Shows all `.esm` / `.esl` / `.esp` files found in the game's `Data`
/// directory, ordered by `ModDatabase::get_ordered_plugins`.  Vanilla masters
/// are pinned at the top and cannot be moved.
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

    // Sort button – sorts non-vanilla plugins by type (ESM → ESL → ESP) then
    // alphabetically, matching the baseline LOOT sorting strategy.
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
        Some(g) => {
            sort_btn.set_tooltip_text(Some(
                "Sort plugins by type: ESM → ESL → ESP, then alphabetically",
            ));
            let game_rc = Rc::new(g.clone());
            let search_query = Rc::new(RefCell::new(String::new()));

            {
                let game_c = Rc::clone(&game_rc);
                let container_c = content_container.clone();
                let search_c = Rc::clone(&search_query);
                search_entry.connect_search_changed(move |entry| {
                    *search_c.borrow_mut() = entry.text().to_string();
                    refresh_load_order_content_with_search(
                        &container_c,
                        &game_c,
                        &search_c.borrow(),
                    );
                });
            }

            // Connect sort button
            {
                let game_c = Rc::clone(&game_rc);
                let container_c = content_container.clone();
                let search_c = Rc::clone(&search_query);
                sort_btn.connect_clicked(move |_| {
                    let mut db = ModDatabase::load(&game_c);
                    if db.plugin_load_order.is_empty() {
                        db.sync_from_plugins_txt(&game_c);
                    }
                    db.sort_plugins_by_type(&game_c);
                    db.save(&game_c);
                    if let Err(e) = db.write_plugins_txt(&game_c) {
                        log::warn!("Failed to write plugins.txt after sort: {e}");
                    }
                    refresh_load_order_content_with_search(
                        &container_c,
                        &game_c,
                        &search_c.borrow(),
                    );
                });
            }

            refresh_load_order_content_with_search(
                &content_container,
                &game_rc,
                &search_query.borrow(),
            );
        }
    };

    toolbar_view.set_content(Some(&content_container));
    toolbar_view.upcast()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Re-populate `container` with the current plugin list for `game`.
fn refresh_load_order_content(container: &gtk4::Box, game: &Rc<Game>) {
    refresh_load_order_content_with_search(container, game, "");
}

fn refresh_load_order_content_with_search(
    container: &gtk4::Box,
    game: &Rc<Game>,
    search_query: &str,
) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let mut db = ModDatabase::load(game);
    // Initialise from plugins.txt if we have no saved order yet
    if db.plugin_load_order.is_empty() {
        db.sync_from_plugins_txt(game);
    }
    let plugins = db.get_ordered_plugins(game);

    if plugins.is_empty() {
        let status = adw::StatusPage::builder()
            .title("No Plugins Found")
            .description(
                "No .esm / .esl / .esp files were found in the game's Data directory.\n\
                 Install and deploy mods first, or check that the game path is correct.",
            )
            .icon_name("format-justify-left-symbolic")
            .build();
        status.set_vexpand(true);
        container.append(&status);
        return;
    }

    let filtered: Vec<PluginFile> = plugins
        .into_iter()
        .filter(|p| {
            matches_query(&p.name, search_query) || matches_query(p.kind.label(), search_query)
        })
        .collect();

    if filtered.is_empty() {
        let status = adw::StatusPage::builder()
            .title("No Matching Plugins")
            .description("No plugins match your search.")
            .icon_name("system-search-symbolic")
            .build();
        status.set_vexpand(true);
        container.append(&status);
        return;
    }

    let count = filtered.len();
    let vanilla_count = filtered.iter().filter(|p| p.is_vanilla).count();
    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);

    for (idx, plugin) in filtered.iter().enumerate() {
        let row = build_plugin_row(plugin, idx, count, vanilla_count, game, container);
        list_box.append(&row);
    }

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

    container.append(&scrolled);
}

fn build_plugin_row(
    plugin: &PluginFile,
    idx: usize,
    total: usize,
    vanilla_count: usize,
    game: &Rc<Game>,
    container: &gtk4::Box,
) -> adw::ActionRow {
    // Subtitle: type label + vanilla marker
    let subtitle = if plugin.is_vanilla {
        format!("{} · Vanilla master (pinned)", plugin.kind.label())
    } else {
        plugin.kind.label().to_string()
    };

    let row = adw::ActionRow::builder()
        .title(&plugin.name)
        .subtitle(&subtitle)
        .build();

    // Drag handle (non-vanilla only) – shown at the far left to indicate draggability
    if !plugin.is_vanilla {
        let drag_handle = gtk4::Image::from_icon_name("list-drag-handle-symbolic");
        drag_handle.add_css_class("dim-label");
        drag_handle.set_tooltip_text(Some("Drag to reorder"));
        row.add_prefix(&drag_handle);
    }

    // Index prefix
    let index_label = gtk4::Label::new(Some(&format!("{}", idx + 1)));
    index_label.add_css_class("dim-label");
    index_label.add_css_class("numeric");
    index_label.set_width_chars(3);
    row.add_prefix(&index_label);

    // Type badge
    let badge = gtk4::Label::new(Some(plugin.kind.label()));
    badge.add_css_class("caption");
    badge.add_css_class("dim-label");
    badge.set_valign(gtk4::Align::Center);
    row.add_suffix(&badge);

    if plugin.is_vanilla {
        // Vanilla masters: no controls, just a lock icon
        let lock = gtk4::Image::from_icon_name("changes-prevent-symbolic");
        lock.set_tooltip_text(Some("Vanilla master – cannot be moved or disabled"));
        lock.add_css_class("dim-label");
        row.add_suffix(&lock);
        return row;
    }

    // Enable/disable toggle
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
        let container_c = container.clone();
        let plugin_name = plugin.name.clone();
        enabled_btn.connect_toggled(move |btn| {
            let enabled = btn.is_active();
            let mut db = ModDatabase::load(&game_c);
            if enabled {
                db.plugin_disabled.remove(&plugin_name);
            } else {
                db.plugin_disabled.insert(plugin_name.clone());
            }
            db.save(&game_c);
            let _ = db.write_plugins_txt(&game_c);
            refresh_load_order_content(&container_c, &game_c);
        });
    }
    row.add_suffix(&enabled_btn);

    // Move up / move down buttons (non-vanilla only)
    let up_btn = gtk4::Button::new();
    up_btn.set_icon_name("go-up-symbolic");
    up_btn.set_valign(gtk4::Align::Center);
    up_btn.add_css_class("flat");
    up_btn.set_tooltip_text(Some("Move up"));
    // Disable when this is the first non-vanilla plugin (can't move into vanilla territory)
    up_btn.set_sensitive(idx > vanilla_count);

    let down_btn = gtk4::Button::new();
    down_btn.set_icon_name("go-down-symbolic");
    down_btn.set_valign(gtk4::Align::Center);
    down_btn.add_css_class("flat");
    down_btn.set_tooltip_text(Some("Move down"));
    down_btn.set_sensitive(idx + 1 < total);

    row.add_suffix(&up_btn);
    row.add_suffix(&down_btn);

    // Up button
    {
        let game_c = Rc::clone(game);
        let container_c = container.clone();
        let plugin_name = plugin.name.clone();
        up_btn.connect_clicked(move |_| {
            let mut db = ModDatabase::load(&game_c);
            let mut ordered = db.get_ordered_plugins(&game_c);
            // Only move within the non-vanilla section
            if let Some(pos) = ordered
                .iter()
                .position(|p| p.name == plugin_name && !p.is_vanilla)
            {
                if pos > 0 && !ordered[pos - 1].is_vanilla {
                    ordered.swap(pos, pos - 1);
                    db.set_plugin_order(&ordered);
                    db.save(&game_c);
                    let _ = db.write_plugins_txt(&game_c);
                    refresh_load_order_content(&container_c, &game_c);
                }
            }
        });
    }

    // Down button
    {
        let game_c = Rc::clone(game);
        let container_c = container.clone();
        let plugin_name = plugin.name.clone();
        down_btn.connect_clicked(move |_| {
            let mut db = ModDatabase::load(&game_c);
            let mut ordered = db.get_ordered_plugins(&game_c);
            let len = ordered.len();
            if let Some(pos) = ordered
                .iter()
                .position(|p| p.name == plugin_name && !p.is_vanilla)
            {
                if pos + 1 < len && !ordered[pos + 1].is_vanilla {
                    ordered.swap(pos, pos + 1);
                    db.set_plugin_order(&ordered);
                    db.save(&game_c);
                    let _ = db.write_plugins_txt(&game_c);
                    refresh_load_order_content(&container_c, &game_c);
                }
            }
        });
    }

    // ── Drag-and-drop ─────────────────────────────────────────────────────────

    // DragSource: let the user drag this row to a new position
    let drag_source = gtk4::DragSource::new();
    drag_source.set_actions(gdk::DragAction::MOVE);
    {
        let plugin_name_drag = plugin.name.clone();
        drag_source.connect_prepare(move |_, _, _| {
            let value = plugin_name_drag.to_value();
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

    // DropTarget: accept a dragged plugin name and move it here
    let drop_target = gtk4::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
    {
        let game_drop = Rc::clone(game);
        let container_drop = container.clone();
        let target_name = plugin.name.clone();
        drop_target.connect_drop(move |_, value, _, _| {
            let Ok(source_name) = value.get::<String>() else {
                return false;
            };
            if source_name == target_name {
                return false;
            }
            let mut db = ModDatabase::load(&game_drop);
            let mut ordered = db.get_ordered_plugins(&game_drop);
            if let (Some(src_pos), Some(tgt_pos)) = (
                ordered.iter().position(|p| p.name == source_name),
                ordered.iter().position(|p| p.name == target_name),
            ) {
                if !ordered[src_pos].is_vanilla && !ordered[tgt_pos].is_vanilla {
                    let plugin_to_move = ordered.remove(src_pos);
                    // After removal the indices above src_pos shift down by one
                    let insert_pos = adjusted_insert_pos(src_pos, tgt_pos, &ordered);
                    ordered.insert(insert_pos, plugin_to_move);
                    db.set_plugin_order(&ordered);
                    db.save(&game_drop);
                    let _ = db.write_plugins_txt(&game_drop);
                    refresh_load_order_content(&container_drop, &game_drop);
                }
            }
            true
        });
    }
    row.add_controller(drop_target);

    // ── Right-click context menu ──────────────────────────────────────────────
    let right_click = gtk4::GestureClick::new();
    right_click.set_button(3); // right mouse button
    {
        let row_c = row.clone();
        let game_rclick = Rc::clone(game);
        let container_rclick = container.clone();
        let plugin_name_rclick = plugin.name.clone();
        let current_idx = idx;
        let vanilla_count_rclick = vanilla_count;
        let total_rclick = total;

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

            let move_item = gtk4::Button::with_label("Move to Position\u{2026}");
            move_item.add_css_class("flat");
            move_item.set_halign(gtk4::Align::Fill);
            move_item.set_hexpand(true);
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
            let container_btn = container_rclick.clone();
            let plugin_name_btn = plugin_name_rclick.clone();

            move_item.connect_clicked(move |_| {
                popover_c.popdown();
                if let Some(root) = row_btn.root() {
                    if let Ok(window) = root.downcast::<gtk4::Window>() {
                        show_move_to_position_dialog(
                            &window,
                            plugin_name_btn.clone(),
                            current_idx,
                            vanilla_count_rclick,
                            total_rclick,
                            Rc::clone(&game_btn),
                            container_btn.clone(),
                        );
                    }
                }
            });

            let popover_enable = popover.clone();
            let game_enable = Rc::clone(&game_rclick);
            let container_enable = container_rclick.clone();
            enable_all_item.connect_clicked(move |_| {
                popover_enable.popdown();
                let mut db = ModDatabase::load(&game_enable);
                db.plugin_disabled.clear();
                db.save(&game_enable);
                let _ = db.write_plugins_txt(&game_enable);
                refresh_load_order_content(&container_enable, &game_enable);
            });

            let popover_disable = popover.clone();
            let game_disable = Rc::clone(&game_rclick);
            let container_disable = container_rclick.clone();
            disable_all_item.connect_clicked(move |_| {
                popover_disable.popdown();
                let mut db = ModDatabase::load(&game_disable);
                let ordered = db.get_ordered_plugins(&game_disable);
                for p in &ordered {
                    if !p.is_vanilla && !db.plugin_disabled.contains(&p.name) {
                        db.plugin_disabled.insert(p.name.clone());
                    }
                }
                db.save(&game_disable);
                let _ = db.write_plugins_txt(&game_disable);
                refresh_load_order_content(&container_disable, &game_disable);
            });

            popover.popup();
        });
    }
    row.add_controller(right_click);

    row
}

// ── Move-to-Position dialog ───────────────────────────────────────────────────

/// Compute the insertion index after removing an element from `ordered`.
///
/// When `src_pos` is removed, all indices above it shift down by one.  The
/// returned `insert_pos` maps the original `target_idx` to its correct
/// post-removal slot, clamped so it never falls inside the vanilla region.
fn adjusted_insert_pos(src_pos: usize, target_idx: usize, ordered: &[PluginFile]) -> usize {
    let raw = if src_pos < target_idx {
        target_idx - 1
    } else {
        target_idx
    };
    let first_non_vanilla = ordered
        .iter()
        .position(|p| !p.is_vanilla)
        .unwrap_or(ordered.len());
    raw.max(first_non_vanilla).min(ordered.len())
}

fn matches_query(value: &str, query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return true;
    }
    value.to_lowercase().contains(&trimmed.to_lowercase())
}

/// Show a modal dialog that lets the user type a load-order position number for
/// `plugin_name`.  Vanilla masters (positions 1–`vanilla_count`) are protected;
/// the valid input range is `vanilla_count + 1` to `total`.
fn show_move_to_position_dialog(
    parent: &gtk4::Window,
    plugin_name: String,
    current_idx: usize,
    vanilla_count: usize,
    total: usize,
    game: Rc<Game>,
    container: gtk4::Box,
) {
    let min_pos = vanilla_count + 1;

    let body = if vanilla_count > 0 {
        format!(
            "Enter the new load order position for \"{plugin_name}\".\n\
             Valid range: {min_pos}–{total} (positions 1–{vanilla_count} are vanilla masters).",
        )
    } else {
        format!(
            "Enter the new load order position for \"{plugin_name}\".\n\
             Valid range: 1–{total}.",
        )
    };

    let dialog = adw::AlertDialog::builder()
        .heading("Move to Position")
        .body(&body)
        .build();

    let spin = gtk4::SpinButton::with_range(min_pos as f64, total as f64, 1.0);
    spin.set_value((current_idx + 1) as f64);
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
        // Convert to 0-indexed
        let target_idx = target_pos_1indexed.saturating_sub(1);

        let mut db = ModDatabase::load(&game);
        let mut ordered = db.get_ordered_plugins(&game);

        if let Some(src_pos) = ordered.iter().position(|p| p.name == plugin_name) {
            if !ordered[src_pos].is_vanilla && target_idx < ordered.len() {
                let p = ordered.remove(src_pos);
                let insert_pos = adjusted_insert_pos(src_pos, target_idx, &ordered);
                ordered.insert(insert_pos, p);
                db.set_plugin_order(&ordered);
                db.save(&game);
                let _ = db.write_plugins_txt(&game);
                refresh_load_order_content(&container, &game);
            }
        }
    });

    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::{adjusted_insert_pos, matches_query};
    use crate::core::mods::{PluginFile, PluginKind};

    #[test]
    fn adjusted_insert_pos_never_enters_vanilla_region() {
        let ordered = vec![
            PluginFile {
                name: "Skyrim.esm".to_string(),
                kind: PluginKind::Master,
                enabled: true,
                is_vanilla: true,
            },
            PluginFile {
                name: "A.esp".to_string(),
                kind: PluginKind::Plugin,
                enabled: true,
                is_vanilla: false,
            },
        ];
        assert_eq!(adjusted_insert_pos(1, 0, &ordered), 1);
    }

    #[test]
    fn matches_query_is_case_insensitive() {
        assert!(matches_query("MyPatch.esp", "patch"));
        assert!(matches_query("MyPatch.esp", "  ESP  "));
        assert!(!matches_query("MyPatch.esp", "armor"));
    }
}
