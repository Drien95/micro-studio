//! Icône dans la zone de notification (près de l'horloge), Windows seulement.
//!
//! Fermer la fenêtre la cache : le micro traité reste disponible pour Discord
//! ou les jeux. Un clic sur l'icône la rouvre ; son menu propose « Ouvrir » et
//! « Quitter ». Les clics arrivent dans le fil de la boucle d'événements, en
//! dehors de l'affichage d'egui : on y envoie directement les commandes de
//! fenêtre, ce qui marche même quand la fenêtre est cachée.

use eframe::egui;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Taille de l'icône dessinée (pixels).
pub const ICON_SIZE: u32 = 32;

/// Icône « micro » dessinée en code (RGBA, `size` × `size`) : une capsule
/// pleine sur son pied, sur fond transparent, lisible à 16 et 32 pixels.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    let (cx, r) = (s / 2.0, s * 0.19);
    let (top, bottom) = (s * 0.08 + r, s * 0.56 - r * 0.2);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Capsule : distance au segment vertical [top, bottom].
            let dy = fy - fy.clamp(top, bottom);
            let capsule = ((fx - cx).powi(2) + dy.powi(2)).sqrt() <= r;
            // Arceau autour de la capsule, puis pied et socle.
            let ring_r = ((fx - cx).powi(2) + (fy - bottom).powi(2)).sqrt();
            let arc = fy >= bottom && (ring_r - r * 1.65).abs() <= s * 0.045;
            let stem = (fx - cx).abs() <= s * 0.045 && fy >= bottom + r * 1.6 && fy <= s * 0.88;
            let base = (fx - cx).abs() <= s * 0.2 && (fy - s * 0.88).abs() <= s * 0.04;
            if capsule || arc || stem || base {
                let i = ((y * size + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[0x3d, 0xa5, 0xf4, 0xff]);
            }
        }
    }
    px
}

/// État partagé avec les gestionnaires de clics de l'icône.
#[derive(Debug, Default)]
pub struct TrayFlags {
    quit: AtomicBool,
}

impl TrayFlags {
    /// « Quitter » a-t-il été choisi dans le menu ?
    pub fn quit_requested(&self) -> bool {
        self.quit.load(Ordering::Relaxed)
    }

    pub fn request_quit(&self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

/// Réaffiche la fenêtre cachée et la met au premier plan.
pub fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

/// Ferme vraiment l'application (choix « Quitter »).
pub fn quit(ctx: &egui::Context, flags: &TrayFlags) {
    flags.request_quit();
    // La fenêtre est réaffichée pour que sa boucle d'affichage tourne et
    // traite la fermeture, puis enregistre les réglages.
    show_window(ctx);
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

/// L'icône vivante ; la supprimer retire l'icône de la barre.
pub struct Tray {
    flags: Arc<TrayFlags>,
    #[cfg(windows)]
    _icon: tray_icon::TrayIcon,
}

impl std::fmt::Debug for Tray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tray").field("flags", &self.flags).finish_non_exhaustive()
    }
}

impl Tray {
    pub fn flags(&self) -> &Arc<TrayFlags> {
        &self.flags
    }

    /// Crée l'icône. À appeler depuis le fil de la fenêtre, une fois la
    /// boucle d'événements lancée (création de l'application eframe).
    #[cfg(windows)]
    pub fn new(ctx: &egui::Context) -> Result<Self, String> {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
        use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

        let open = MenuItem::new("Ouvrir Micro Studio", true, None);
        let exit = MenuItem::new("Quitter", true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &PredefinedMenuItem::separator(), &exit]).map_err(|e| e.to_string())?;
        let icon = Icon::from_rgba(icon_rgba(ICON_SIZE), ICON_SIZE, ICON_SIZE).map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("Micro Studio : le micro traité reste actif")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())?;

        let flags = Arc::new(TrayFlags::default());
        let (c, f) = (ctx.clone(), Arc::clone(&flags));
        let (open_id, exit_id) = (open.id().clone(), exit.id().clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == open_id {
                show_window(&c);
            } else if e.id == exit_id {
                quit(&c, &f);
            }
        }));
        let c = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_window(&c);
            }
        }));
        Ok(Self { flags, _icon: tray })
    }

    /// Hors Windows, pas d'icône : fermer la fenêtre quitte l'application.
    #[cfg(not(windows))]
    pub fn new(_ctx: &egui::Context) -> Result<Self, String> {
        Err("icône de notification disponible sous Windows seulement".into())
    }
}

/// Décide quoi faire d'une demande de fermeture de la fenêtre : `true` s'il
/// faut la cacher au lieu de quitter.
pub fn hide_instead_of_close(has_tray: bool, quit_on_close: bool, quit_requested: bool) -> bool {
    has_tray && !quit_on_close && !quit_requested
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_the_right_size_and_a_visible_shape() {
        for size in [16, 32, 64] {
            let px = icon_rgba(size);
            assert_eq!(px.len(), (size * size * 4) as usize);
            let opaque = px.chunks(4).filter(|p| p[3] == 0xff).count() as f32 / (size * size) as f32;
            assert!((0.1..0.5).contains(&opaque), "{size} px : {opaque:.2} de pixels pleins");
            // Symétrique gauche / droite.
            for y in 0..size {
                for x in 0..size / 2 {
                    let a = ((y * size + x) * 4 + 3) as usize;
                    let b = ((y * size + size - 1 - x) * 4 + 3) as usize;
                    assert_eq!(px[a], px[b], "({x},{y}) à {size} px");
                }
            }
        }
    }

    #[test]
    fn closing_hides_only_with_a_tray_and_without_quit() {
        assert!(hide_instead_of_close(true, false, false));
        assert!(!hide_instead_of_close(false, false, false), "sans icône, on ne pourrait plus rouvrir");
        assert!(!hide_instead_of_close(true, true, false), "choix « quitter à la fermeture »");
        assert!(!hide_instead_of_close(true, false, true), "« Quitter » dans le menu");
    }

    #[test]
    fn quit_sets_the_flag() {
        let ctx = egui::Context::default();
        let f = TrayFlags::default();
        assert!(!f.quit_requested());
        quit(&ctx, &f);
        assert!(f.quit_requested());
    }
}
