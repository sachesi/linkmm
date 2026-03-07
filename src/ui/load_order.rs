use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::core::games::Game;
use crate::core::mods::ModDatabase;

/// Build the Load Order page for `game`.
///
/// If `game` is `None`, shows an "no game selected" placeholder.
pub fn build_load_order_page(game: Option<&Game>) -> gtk4::Widget {
    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let title_widget = match game {
        Some(g) => adw::WindowTitle::new("Load Order", &g.name),
        None => adw::WindowTitle::new("Load Order", ""),
    };
    header.set_title_widget(Some(&title_widget));

    toolbar_view.add_top_bar(&header);

    let content_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_container.set_vexpand(true);

    match game {
        None => {
            let status = adw::StatusPage::builder()
                .title("No Game Selected")
                .description("Select a game from the sidebar to manage its load order.")
                .icon_name("applications-games-symbolic")
                .build();
            status.set_vexpand(true);
            content_container.append(&status);
        }
        Some(g) => {
            let game_rc = Rc::new(g.clone());
            refresh_load_order_content(&content_container, &game_rc);
        }
    };

    toolbar_view.set_content(Some(&content_container));
    toolbar_view.upcast()
}

/// Re-populate `container` from the current mod database for `game`.
fn refresh_load_order_content(container: &gtk4::Box, game: &Rc<Game>) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let db = ModDatabase::load(game);

    if db.load_order.is_empty() && db.mods.is_empty() {
        let status = adw::StatusPage::builder()
            .title("No Mods in Load Order")
            .description("Add mods in the Library tab to see them here.")
            .icon_name("format-justify-left-symbolic")
            .build();
        status.set_vexpand(true);
        container.append(&status);
        return;
    }

    let ordered_ids = compute_ordered_ids(&db);
    let count = ordered_ids.len();

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);

    for (idx, mod_id) in ordered_ids.iter().enumerate() {
        let Some(mod_entry) = db.mods.iter().find(|m| &m.id == mod_id) else {
            continue;
        };

        let row = adw::ActionRow::builder()
            .title(&mod_entry.name)
            .subtitle(if mod_entry.enabled { "Enabled" } else { "Disabled" })
            .build();

        // Index prefix
        let index_label = gtk4::Label::new(Some(&format!("{}", idx + 1)));
        index_label.add_css_class("dim-label");
        index_label.add_css_class("numeric");
        index_label.set_width_chars(3);
        row.add_prefix(&index_label);

        // Move-up / move-down buttons
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
        down_btn.set_sensitive(idx + 1 < count);

        row.add_suffix(&up_btn);
        row.add_suffix(&down_btn);

        // Connect up button — re-reads DB so position is always current
        {
            let game_c = Rc::clone(game);
            let container_c = container.clone();
            let mod_id_c = mod_id.clone();
            up_btn.connect_clicked(move |_| {
                let mut db = ModDatabase::load(&game_c);
                let mut order = compute_ordered_ids(&db);
                if let Some(pos) = order.iter().position(|id| id == &mod_id_c) {
                    if pos > 0 {
                        order.swap(pos, pos - 1);
                        db.load_order = order;
                        db.save(&game_c);
                        refresh_load_order_content(&container_c, &game_c);
                    }
                }
            });
        }

        // Connect down button — re-reads DB so position is always current
        {
            let game_c = Rc::clone(game);
            let container_c = container.clone();
            let mod_id_c = mod_id.clone();
            down_btn.connect_clicked(move |_| {
                let mut db = ModDatabase::load(&game_c);
                let mut order = compute_ordered_ids(&db);
                let len = order.len();
                if let Some(pos) = order.iter().position(|id| id == &mod_id_c) {
                    if pos + 1 < len {
                        order.swap(pos, pos + 1);
                        db.load_order = order;
                        db.save(&game_c);
                        refresh_load_order_content(&container_c, &game_c);
                    }
                }
            });
        }

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

/// Return mod IDs in load-order sequence.
/// Uses `db.load_order` when populated; falls back to the natural mod list order.
/// Any mods not referenced in `load_order` are appended at the end.
fn compute_ordered_ids(db: &ModDatabase) -> Vec<String> {
    if db.load_order.is_empty() {
        return db.mods.iter().map(|m| m.id.clone()).collect();
    }
    let mut result: Vec<String> = db
        .load_order
        .iter()
        .filter(|id| db.mods.iter().any(|m| &m.id == *id))
        .cloned()
        .collect();
    for m in &db.mods {
        if !result.contains(&m.id) {
            result.push(m.id.clone());
        }
    }
    result
}
