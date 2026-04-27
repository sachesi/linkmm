use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use glib;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::core::config::AppConfig;
use crate::core::games::Game;
use crate::core::mods::ModDatabase;
use crate::ui::ordering;

mod conflicts;
mod mod_row;

use conflicts::compute_conflict_states;
use mod_row::{DndRowData, build_mod_row};


const CLAMP_MARGIN: f64 = 12.0;
const SCROLL_EDGE: f64 = 60.0;
const SCROLL_SPEED: f64 = 12.0;

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

    toolbar_view.add_top_bar(&header);

    let content_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_container.set_vexpand(true);

    let list_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    list_container.set_vexpand(true);
    let reorder_hint = gtk4::Label::new(Some("Clear search to reorder."));
    reorder_hint.add_css_class("dim-label");
    reorder_hint.set_margin_top(8);
    reorder_hint.set_margin_bottom(4);
    reorder_hint.set_margin_start(16);
    reorder_hint.set_margin_end(16);
    reorder_hint.set_halign(gtk4::Align::Start);
    reorder_hint.set_visible(false);
    content_container.append(&reorder_hint);
    content_container.append(&list_container);

    let game_rc = Rc::new(game.clone());
    let search_query = Rc::new(RefCell::new(String::new()));
    let selected_mod_id = Rc::new(RefCell::new(None::<String>));

    refresh_library_content_with_search(
        &list_container,
        &game_rc,
        Rc::clone(&config),
        &search_query.borrow(),
        Rc::clone(&search_query),
        Rc::clone(&selected_mod_id),
        &reorder_hint,
        true,
    );

    toolbar_view.set_content(Some(&content_container));

    {
        let container_c = list_container.clone();
        let game_c = Rc::clone(&game_rc);
        let config_c = Rc::clone(&config);
        let search_c = Rc::clone(&search_query);
        let selected_c = Rc::clone(&selected_mod_id);
        let reorder_hint_c = reorder_hint.clone();
        search_entry.connect_search_changed(move |entry| {
            *search_c.borrow_mut() = entry.text().to_string();
            refresh_library_content_with_search(
                &container_c,
                &game_c,
                Rc::clone(&config_c),
                &search_c.borrow(),
                Rc::clone(&search_c),
                Rc::clone(&selected_c),
                &reorder_hint_c,
                false,
            );
        });
    }

    toolbar_view.upcast()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(crate) fn refresh_library_content_with_search(
    container: &gtk4::Box,
    game: &Rc<Game>,
    config: Rc<RefCell<AppConfig>>,
    search_query: &str,
    search_state: Rc<RefCell<String>>,
    selected_mod_id: Rc<RefCell<Option<String>>>,
    reorder_hint: &gtk4::Label,
    do_scan: bool,
) {
    let previous_scroll = container
        .first_child()
        .and_then(|child| child.downcast::<gtk4::ScrolledWindow>().ok())
        .map(|scrolled| {
            let adj = scrolled.vadjustment();
            (adj.value(), adj.upper(), adj.page_size())
        });

    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

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

    let reorder_allowed = search_query.trim().is_empty();
    reorder_hint.set_visible(!reorder_allowed);

    let visible_mods: Vec<_> = db
        .mods
        .iter()
        .enumerate()
        .filter(|(_, m)| matches_query(&m.name, search_query))
        .map(|(idx, m)| (idx, m.clone()))
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

        let mut dnd_rows: Vec<DndRowData> = Vec::with_capacity(visible_mods.len());

        for (full_idx, mod_entry) in &visible_mods {
            let result = build_mod_row(
                mod_entry,
                *full_idx,
                db.mods.len(),
                reorder_allowed,
                game,
                container,
                Rc::clone(&config),
                Rc::clone(&search_state),
                Rc::clone(&selected_mod_id),
                reorder_hint,
                conflict_states.get(&mod_entry.id),
            );
            list_box.append(&result.row);
            dnd_rows.push(DndRowData {
                mod_id: mod_entry.id.clone(),
                row: result.row,
                index_label: result.index_label,
                up_btn: result.up_btn,
                down_btn: result.down_btn,
            });
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

        if reorder_allowed {
            let dnd_rows_rc = Rc::new(RefCell::new(dnd_rows));
            setup_library_dnd(&list_box, &scrolled, dnd_rows_rc, Rc::clone(game));
        }

        if let Some((value, _, _)) = previous_scroll {
            let scrolled_clone = scrolled.clone();
            gtk4::glib::idle_add_local_once(move || {
                let adj = scrolled_clone.vadjustment();
                let max_value = (adj.upper() - adj.page_size()).max(0.0);
                adj.set_value(value.clamp(0.0, max_value));
            });
        }
    }
}

// ── Drag & drop for the library list ─────────────────────────────────────────

fn clear_lib_dnd_indicator(indicator: &Rc<RefCell<Option<(gtk4::Widget, bool)>>>) {
    if let Some((w, is_before)) = indicator.borrow_mut().take() {
        if is_before {
            w.remove_css_class("dnd-drop-before");
        } else {
            w.remove_css_class("dnd-drop-after");
        }
    }
}

fn setup_library_dnd(
    list_box: &gtk4::ListBox,
    scrolled: &gtk4::ScrolledWindow,
    dnd_rows: Rc<RefCell<Vec<DndRowData>>>,
    game: Rc<Game>,
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
            clear_lib_dnd_indicator(&indicator_l);
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
            clear_lib_dnd_indicator(&indicator_d);

            let Ok(source_id) = value.get::<String>() else {
                return false;
            };

            let adj = scrolled_d.vadjustment();
            let listbox_y = (y + adj.value() - CLAMP_MARGIN) as i32;

            let mut rows = dnd_rows_d.borrow_mut();
            let total = rows.len();

            let Some(source_idx) = rows.iter().position(|r| r.mod_id == source_id) else {
                return false;
            };

            let target_idx = match list_box_d.row_at_y(listbox_y) {
                Some(ref trow) => {
                    let tptr = trow.upcast_ref::<glib::Object>().as_ptr() as usize;
                    let row_pos = rows
                        .iter()
                        .position(|r| {
                            r.row.upcast_ref::<glib::Object>().as_ptr() as usize == tptr
                        })
                        .unwrap_or(total);
                    let before = trow
                        .compute_bounds(&list_box_d)
                        .map_or(true, |b| listbox_y < (b.y() + b.height() / 2.0) as i32);
                    if before { row_pos } else { row_pos + 1 }
                }
                None => total,
            };

            if target_idx == source_idx || target_idx == source_idx + 1 {
                return true;
            }

            let insert_idx = ordering::insertion_index_after_removal(source_idx, target_idx);

            let saved_scroll = adj.value();

            let src_widget = rows[source_idx].row.clone().upcast::<gtk4::Widget>();
            list_box_d.remove(&src_widget);
            list_box_d.insert(&src_widget, insert_idx as i32);

            let row_data = rows.remove(source_idx);
            rows.insert(insert_idx, row_data);

            let n = rows.len();
            for (i, r) in rows.iter().enumerate() {
                r.index_label.set_text(&format!("{}", i + 1));
                r.up_btn.set_sensitive(i > 0);
                r.down_btn.set_sensitive(i + 1 < n);
            }

            let new_order: Vec<String> = rows.iter().map(|r| r.mod_id.clone()).collect();
            drop(rows);
            let game_c = Rc::clone(&game_d);
            // Restore scroll after GTK's DnD cleanup (which fires after this handler
            // returns) resets the adjustment. Two nested idles cover two layout passes.
            glib::idle_add_local_once(move || {
                adj.set_value(saved_scroll);
                let adj2 = adj.clone();
                glib::idle_add_local_once(move || { adj2.set_value(saved_scroll); });
            });
            glib::idle_add_local_once(move || {
                let mut db = ModDatabase::load(&game_c);
                let mut new_mods = Vec::with_capacity(db.mods.len());
                for id in &new_order {
                    if let Some(m) = db.mods.iter().find(|m| &m.id == id) {
                        new_mods.push(m.clone());
                    }
                }
                for m in &db.mods {
                    if !new_mods.iter().any(|nm| nm.id == m.id) {
                        new_mods.push(m.clone());
                    }
                }
                db.mods = new_mods;
                db.save(&game_c);
            });

            true
        });
    }

    scrolled.add_controller(drop_target);
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn matches_query(value: &str, query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return true;
    }
    value.to_lowercase().contains(&trimmed.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{compute_conflict_states, matches_query};
    use crate::core::mods::Mod;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_mod(id: &str, name: &str, path: &str) -> Mod {
        let mut m = Mod::new(name.to_string(), PathBuf::from(path));
        m.id = id.to_string();
        m
    }

    #[test]
    fn matches_query_is_case_insensitive() {
        assert!(matches_query("Immersive Armors", "arm"));
        assert!(matches_query("Immersive Armors", "  ARMORS  "));
        assert!(!matches_query("Immersive Armors", "weapons"));
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
        let root =
            std::env::temp_dir().join(format!("linkmm-conflict-fallback-test-{unique}"));
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
