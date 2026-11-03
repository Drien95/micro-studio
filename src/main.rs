#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! Point d'entrée : ouvre la fenêtre de Micro Studio.

use micro_studio::{autostart, ui};

fn main() -> eframe::Result {
    // Lancé par Windows à l'ouverture de session : fenêtre cachée, seule
    // l'icône près de l'horloge apparaît (elle n'existe que sous Windows).
    let hidden = cfg!(windows) && autostart::started_minimized(std::env::args());
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Micro Studio")
            .with_inner_size([900.0, 760.0])
            .with_min_inner_size([640.0, 480.0])
            .with_visible(!hidden),
        ..Default::default()
    };
    eframe::run_native("Micro Studio", options, Box::new(|cc| Ok(Box::new(ui::App::new(cc)))))
}
