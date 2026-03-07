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

    let content: gtk4::Widget = match game {
        None => {
            let status = adw::StatusPage::builder()
                .title("No Game Selected")
                .description("Select a game from the sidebar to manage its load order.")
                .icon_name("applications-games-symbolic")
                .build();
            status.set_vexpand(true);
            status.upcast()
        }
        Some(g) => build_load_order_content(g),
    };

    toolbar_view.set_content(Some(&content));
    toolbar_view.upcast()
}

fn build_load_order_content(game: &Game) -> gtk4::Widget {
    let db = ModDatabase::load(game);

    if db.load_order.is_empty() && db.mods.is_empty() {
        let status = adw::StatusPage::builder()
            .title("No Mods in Load Order")
            .description("Add mods in the Library tab to see them here.")
            .icon_name("format-justify-left-symbolic")
            .build();
        status.set_vexpand(true);
        return status.upcast();
    }

    // Build an ordered list: use load_order if populated, otherwise use mod list order
    let ordered: Vec<&crate::core::mods::Mod> = if db.load_order.is_empty() {
        db.mods.iter().collect()
    } else {
        let mut result: Vec<&crate::core::mods::Mod> = db
            .load_order
            .iter()
            .filter_map(|id| db.mods.iter().find(|m| &m.id == id))
            .collect();
        // Append any mods not in load_order
        for m in &db.mods {
            if !db.load_order.contains(&m.id) {
                result.push(m);
            }
        }
        result
    };

    let list_box = gtk4::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_selection_mode(gtk4::SelectionMode::None);

    for (idx, m) in ordered.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(&m.name)
            .subtitle(if m.enabled { "Enabled" } else { "Disabled" })
            .build();

        let index_label = gtk4::Label::new(Some(&format!("{}", idx + 1)));
        index_label.add_css_class("dim-label");
        index_label.add_css_class("numeric");
        row.add_prefix(&index_label);

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

    scrolled.upcast()
}
