//! # Micro Studio
//!
//! Traitement du micro en temps réel avec calibrage automatique.
//!
//! La bibliothèque regroupe tout ce qui peut être testé sans carte son :
//! la chaîne d'effets ([`dsp`]), les réglages ([`params`], [`settings`]) et le
//! calibrage ([`calibrate`]). Le moteur temps réel ([`engine`]) et
//! l'interface ([`ui`]) s'appuient dessus ; le binaire `micro-studio` ne fait
//! que lancer la fenêtre.
//!
//! ## Règles temps réel
//!
//! Le code appelé depuis les callbacks audio (`Chain::process`,
//! `Chain::set_params`, les rééchantillonneurs) ne doit ni allouer de mémoire,
//! ni prendre de verrou bloquant, ni faire d'entrée/sortie. Le test
//! d'intégration `tests/realtime.rs` vérifie l'absence d'allocation.

pub mod autostart;
pub mod calibrate;
pub mod dnsmos;
pub mod dsp;
pub mod engine;
pub mod monitor;
pub mod params;
pub mod settings;
pub mod tray;
pub mod ui;
pub mod vbcable;
