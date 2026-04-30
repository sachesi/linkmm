pub mod ui;
pub use linkmm::core;

use gio::prelude::FileExt;
use gtk4::prelude::{ApplicationExt, ApplicationExtManual, GtkApplicationExt};

fn main() {
    if let Err(e) = linkmm::core::logger::init() {
        eprintln!("Failed to initialise logger: {e}");
    }

    let mut args = std::env::args().skip(1);
    if let Some(flag) = args.next() {
        if flag == "--steam-session" {
            let Some(app_id_raw) = args.next() else {
                eprintln!("Missing app id for --steam-session");
                std::process::exit(2);
            };
            let mut game_id: Option<String> = None;
            while let Some(arg) = args.next() {
                if arg == "--game-id" {
                    let Some(value) = args.next() else {
                        eprintln!("Missing value for --game-id");
                        std::process::exit(2);
                    };
                    game_id = Some(value);
                } else {
                    eprintln!("Unknown argument for --steam-session: {arg}");
                    std::process::exit(2);
                }
            }
            let app_id = match app_id_raw.parse::<u32>() {
                Ok(value) => value,
                Err(e) => {
                    eprintln!("Invalid app id '{app_id_raw}': {e}");
                    std::process::exit(2);
                }
            };
            match linkmm::core::runtime::run_phase1_steam_session(app_id, game_id.as_deref()) {
                Ok(code) => std::process::exit(code),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
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
