use std::cell::RefCell;
use std::rc::Rc;

use gio;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

pub(super) fn build_app_dir_page(
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

    let path_group = adw::PreferencesGroup::builder().title("Directory").build();

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
            let parent = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok());
            let row_c = dir_row_c.clone();
            let app_dir_cc = Rc::clone(&app_dir_c);
            file_dialog.select_folder(parent.as_ref(), None::<&gio::Cancellable>, move |result| {
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    row_c.set_text(&path.to_string_lossy());
                    *app_dir_cc.borrow_mut() = Some(path);
                }
            });
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
