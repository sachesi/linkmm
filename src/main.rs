mod core {
    pub mod config;
    pub mod deployment;
    pub mod download;
    pub mod download_state;
    pub mod games;
    pub mod installer;
    pub mod installer_new;
    pub mod logger;
    pub mod mods;
    pub mod nexus;
    pub mod nxm;
    pub mod steam;
}
pub mod ui;

use gio;
use gio::prelude::FileExt;
use gtk4::prelude::{ApplicationExt, ApplicationExtManual, GtkApplicationExt};

fn main() {
    if let Err(e) = crate::core::logger::init() {
        eprintln!("Failed to initialise logger: {e}");
    }

    let app = libadwaita::Application::builder()
        .application_id("io.github.sachesi.linkmm")
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_activate(|app| {
        ui::build_ui(app);
    });

    app.connect_open(|app, files, _hint| {
        // Ensure window exists
        if app.active_window().is_none() {
            ui::build_ui(app);
        }
        // Process NXM URLs
        for file in files {
            let uri = file.uri();
            let uri_str = uri.as_str();
            if uri_str.starts_with("nxm://") {
                ui::handle_nxm_url(app, uri_str);
            }
        }
    });

    let exit_code = app.run();
    std::process::exit(exit_code.into());
}
