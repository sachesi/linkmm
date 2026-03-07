mod core {
    pub mod config;
    pub mod games;
    pub mod mods;
    pub mod nexus;
    pub mod steam;
}
pub mod ui;

use gtk4::prelude::{ApplicationExt, ApplicationExtManual};

fn main() {
    env_logger::init();

    let app = libadwaita::Application::builder()
        .application_id("io.github.sachesi.linkmm")
        .build();

    app.connect_activate(|app| {
        ui::build_ui(app);
    });

    let exit_code = app.run();
    std::process::exit(exit_code.into());
}
