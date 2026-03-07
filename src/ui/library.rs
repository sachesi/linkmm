use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::rc::Rc;

use gio;
use gtk4::gdk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::config::AppConfig;
use crate::core::games::Game;
use crate::core::mods::{Mod, ModDatabase, ModManager};

#[derive(Debug, Clone, Default)]
struct ConflictState {
    files: BTreeSet<String>,
}

/// Build the full Library page for `game`.
pub fn build_library_page(game: &Game, config: Rc<RefCell<AppConfig>>) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let title_widget = adw::WindowTitle::new("Library", "");
    header.set_title_widget(Some(&title_widget));

    toolbar_view.add_top_bar(&header);

    let content_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_container.set_vexpand(true);

    let game_rc = Rc::new(game.clone());
    let search_query = Rc::new(RefCell::new(String::new()));
    let selected_mod_id = Rc::new(RefCell::new(None::<String>));

    refresh_library_content_with_search(
        &content_container,
        &game_rc,
        Rc::clone(&config),
        &search_query.borrow(),
        Rc::clone(&search_query),
        Rc::clone(&selected_mod_id),
    );

    toolbar_view.set_content(Some(&content_container));

    toolbar_view.upcast()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn refresh_library_content_with_search(
    container: &gtk4::Box,
    game: &Rc<Game>,
    config: Rc<RefCell<AppConfig>>,
    search_query: &str,
    search_state: Rc<RefCell<String>>,
    selected_mod_id: Rc<RefCell<Option<String>>>,
) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let mut db = ModDatabase::load(game);
    db.scan_mods_dir(game);
    db.save(game);

    if let Some(selected) = selected_mod_id.borrow().as_ref() {
        if !db.mods.iter().any(|m| &m.id == selected) {
            *selected_mod_id.borrow_mut() = None;
        }
    }

    let visible_mods: Vec<_> = db
        .mods
        .iter()
        .filter(|m| matches_query(&m.name, search_query))
        .cloned()
        .collect();

    if visible_mods.is_empty() {
        if !search_query.trim().is_empty() && !db.mods.is_empty() {
            let status = adw::StatusPage::builder()
                .title("No Matching Mods")
                .description("No installed mods match your search.")
                .icon_name("system-search-symbolic")
                .build();
            status.set_vexpand(true);
            container.append(&status);
            return;
        }
        let status = adw::StatusPage::builder()
            .title("No Mods Installed")
            .icon_name("package-x-generic-symbolic")
            .build();
        status.set_vexpand(true);
        container.append(&status);
    } else {
        let selected = selected_mod_id.borrow().clone();
        let conflict_states = compute_conflict_states(&db.mods, selected.as_deref());

        let list_box = gtk4::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk4::SelectionMode::None);

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
                conflict_states.get(&mod_entry.id),
            );
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
}

fn build_mod_row(
    mod_entry: &Mod,
    idx: usize,
    total: usize,
    game: &Rc<Game>,
    container: &gtk4::Box,
    config: Rc<RefCell<AppConfig>>,
    search_state: Rc<RefCell<String>>,
    selected_mod_id: Rc<RefCell<Option<String>>>,
    conflict_state: Option<&ConflictState>,
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title(&mod_entry.name)
        .active(mod_entry.enabled)
        .build();

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
        if !state.files.is_empty() {
            row.add_css_class("accent");
        }
    }

    let mod_id = mod_entry.id.clone();
    let game_clone = Rc::clone(game);

    row.connect_active_notify(move |switch_row| {
        let enabled = switch_row.is_active();
        let mut db = ModDatabase::load(&game_clone);
        if let Some(m) = db.mods.iter_mut().find(|m| m.id == mod_id) {
            m.enabled = enabled;
            db.save(&game_clone);
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
        let mod_id_c = mod_entry.id.clone();
        up_btn.connect_clicked(move |_| {
            let mut db = ModDatabase::load(&game_c);
            if let Some(pos) = db.mods.iter().position(|m| m.id == mod_id_c) {
                if pos > 0 {
                    db.mods.swap(pos, pos - 1);
                    db.save(&game_c);
                    refresh_library_content_with_search(
                        &container_c,
                        &game_c,
                        Rc::clone(&config_c),
                        &search_c.borrow(),
                        Rc::clone(&search_c),
                        Rc::clone(&selected_c),
                    );
                }
            }
        });
    }

    {
        let game_c = Rc::clone(game);
        let container_c = container.clone();
        let config_c = Rc::clone(&config);
        let search_c = Rc::clone(&search_state);
        let selected_c = Rc::clone(&selected_mod_id);
        let mod_id_c = mod_entry.id.clone();
        down_btn.connect_clicked(move |_| {
            let mut db = ModDatabase::load(&game_c);
            let len = db.mods.len();
            if let Some(pos) = db.mods.iter().position(|m| m.id == mod_id_c) {
                if pos + 1 < len {
                    db.mods.swap(pos, pos + 1);
                    db.save(&game_c);
                    refresh_library_content_with_search(
                        &container_c,
                        &game_c,
                        Rc::clone(&config_c),
                        &search_c.borrow(),
                        Rc::clone(&search_c),
                        Rc::clone(&selected_c),
                    );
                }
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
        let target_id = mod_entry.id.clone();
        drop_target.connect_drop(move |_, value, _, _| {
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
                refresh_library_content_with_search(
                    &container_drop,
                    &game_drop,
                    Rc::clone(&config_drop),
                    &search_drop.borrow(),
                    Rc::clone(&search_drop),
                    Rc::clone(&selected_drop),
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
                    let mod_name_lower = m.name.to_lowercase();
                    let mut cfg = config_c.borrow_mut();
                    cfg.installed_archives.retain(|archive| {
                        let archive_stem = Path::new(archive)
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_lowercase())
                            .unwrap_or_default();
                        archive_stem != mod_name_lower
                    });
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
        let mod_id_sel = mod_entry.id.clone();
        left_click.connect_pressed(move |_, _, _, _| {
            {
                let mut selected = selected_sel.borrow_mut();
                if selected.as_ref() == Some(&mod_id_sel) {
                    return;
                }
                *selected = Some(mod_id_sel.clone());
            }
            refresh_library_content_with_search(
                &container_sel,
                &game_sel,
                Rc::clone(&config_sel),
                &search_sel.borrow(),
                Rc::clone(&search_sel),
                Rc::clone(&selected_sel),
            );
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
        let conflict_files = conflict_state
            .map(|state| state.files.iter().cloned().collect::<Vec<_>>())
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
            show_conflicts_item.set_sensitive(!conflict_files.is_empty());
            menu_box.append(&show_conflicts_item);

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
            let conflict_files_for_menu = conflict_files.clone();
            show_conflicts_item.connect_clicked(move |_| {
                popover_conflicts.popdown();
                if conflict_files_for_menu.is_empty() {
                    return;
                }

                let body = conflict_files_for_menu
                    .iter()
                    .map(|f| format!("• {f}"))
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

fn compute_conflict_states(
    mods: &[Mod],
    selected_id: Option<&str>,
) -> HashMap<String, ConflictState> {
    let Some(selected_id) = selected_id else {
        return HashMap::new();
    };

    let Some(selected_idx) = mods.iter().position(|m| m.id == selected_id) else {
        return HashMap::new();
    };

    let selected_files = collect_mod_target_files(&mods[selected_idx]);
    if selected_files.is_empty() {
        return HashMap::new();
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

        states
            .entry(m.id.clone())
            .or_default()
            .files
            .extend(shared.iter().cloned());
        states
            .entry(selected_id.to_string())
            .or_default()
            .files
            .extend(shared.iter().cloned());
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
                } else if path.is_file() {
                    if let Ok(rel) = path.strip_prefix(root) {
                        files.insert(normalize_relative_path("root", rel));
                    }
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
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                files.insert(normalize_relative_path(prefix, rel));
            }
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
        assert!(!states.contains_key("c"));

        std::fs::remove_dir_all(root).unwrap();
    }
}
