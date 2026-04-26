use gtk4::prelude::*;
use libadwaita as adw;

pub(super) fn build_welcome_page() -> gtk4::Box {
    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    page.set_vexpand(true);

    let status = adw::StatusPage::builder()
        .icon_name("applications-games-symbolic")
        .title("Welcome to Linkmm")
        .description(
            "The link-based mod manager for Bethesda games.\n\nLet\u{2019}s set up your first game.",
        )
        .build();
    status.set_vexpand(true);
    page.append(&status);

    let button_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    button_box.set_halign(gtk4::Align::Center);
    button_box.set_margin_bottom(24);

    let get_started_btn = gtk4::Button::with_label("Get Started");
    get_started_btn.add_css_class("suggested-action");
    get_started_btn.add_css_class("pill");
    button_box.append(&get_started_btn);

    page.append(&button_box);
    page
}
