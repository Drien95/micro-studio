#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod calibrate;
mod dsp;
mod engine;
mod params;
mod settings;
mod ui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Micro Studio")
            .with_inner_size([900.0, 760.0])
            .with_min_inner_size([640.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native("Micro Studio", options, Box::new(|cc| Ok(Box::new(ui::App::new(cc)))))
}
