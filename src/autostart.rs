//! Lancement avec Windows : une entrée dans la clé « Run » de l'utilisateur
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`), la méthode que
//! documente Microsoft pour une application sans droits administrateur.
//!
//! Le registre est lu et écrit par `reg.exe`, livré avec Windows : pas de code
//! `unsafe`, pas de dépendance. L'accès passe par le trait [`RunKey`] pour que
//! la logique soit testée sans Windows.

use std::path::Path;

/// Clé du registre lue au démarrage de la session Windows.
pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
/// Nom de notre valeur dans la clé.
pub const VALUE_NAME: &str = "Micro Studio";
/// Argument qui démarre l'application réduite dans la zone de notification.
pub const MINIMIZED_ARG: &str = "--minimized";

/// Accès à notre valeur dans la clé « Run ».
pub trait RunKey {
    /// Commande enregistrée, `None` si absente.
    fn get(&self) -> Option<String>;
    fn set(&mut self, command: &str) -> Result<(), String>;
    fn remove(&mut self) -> Result<(), String>;
}

/// Commande lancée par Windows : l'exécutable entre guillemets (le chemin peut
/// contenir des espaces), puis l'argument de démarrage réduit.
pub fn command_for(exe: &Path) -> String {
    format!("\"{}\" {MINIMIZED_ARG}", exe.display())
}

/// Le lancement avec Windows est-il activé ?
pub fn is_enabled(key: &dyn RunKey) -> bool {
    key.get().is_some()
}

/// Active ou désactive le lancement avec Windows.
pub fn set_enabled(key: &mut dyn RunKey, exe: &Path, on: bool) -> Result<(), String> {
    if on {
        key.set(&command_for(exe))
    } else if key.get().is_some() {
        key.remove()
    } else {
        Ok(())
    }
}

/// Si le lancement est activé mais pointe ailleurs (logiciel déplacé ou mis à
/// jour dans un autre dossier), réécrit le chemin. Renvoie `true` si corrigé.
pub fn refresh(key: &mut dyn RunKey, exe: &Path) -> Result<bool, String> {
    let wanted = command_for(exe);
    match key.get() {
        Some(current) if current != wanted => key.set(&wanted).map(|()| true),
        _ => Ok(false),
    }
}

/// L'application a-t-elle été lancée par Windows à l'ouverture de session ?
pub fn started_minimized(args: impl IntoIterator<Item = String>) -> bool {
    args.into_iter().any(|a| a == MINIMIZED_ARG)
}

/// Extrait la donnée d'une valeur `REG_SZ` de la sortie de `reg query`.
/// Exemple de ligne : `    Micro Studio    REG_SZ    "C:\x.exe" --minimized`.
pub fn parse_query(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix(VALUE_NAME)?;
        let data = rest.trim_start().strip_prefix("REG_SZ")?;
        Some(data.trim().to_owned())
    })
}

/// La vraie clé, via `reg.exe` (Windows seulement).
#[derive(Debug, Default)]
pub struct Registry;

impl Registry {
    fn reg(args: &[&str]) -> Result<String, String> {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let exe = Path::new(&root).join(r"System32\reg.exe");
        let mut cmd = std::process::Command::new(exe);
        cmd.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd.output().map_err(|e| format!("reg.exe introuvable : {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
        }
    }
}

impl RunKey for Registry {
    fn get(&self) -> Option<String> {
        Self::reg(&["query", RUN_KEY, "/v", VALUE_NAME]).ok().as_deref().and_then(parse_query)
    }

    fn set(&mut self, command: &str) -> Result<(), String> {
        Self::reg(&["add", RUN_KEY, "/v", VALUE_NAME, "/t", "REG_SZ", "/d", command, "/f"]).map(|_| ())
    }

    fn remove(&mut self) -> Result<(), String> {
        Self::reg(&["delete", RUN_KEY, "/v", VALUE_NAME, "/f"]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Clé en mémoire, qui peut refuser l'écriture (droits, antivirus…).
    #[derive(Default)]
    struct FakeKey {
        value: Option<String>,
        read_only: bool,
    }

    impl RunKey for FakeKey {
        fn get(&self) -> Option<String> {
            self.value.clone()
        }
        fn set(&mut self, command: &str) -> Result<(), String> {
            if self.read_only {
                return Err("Accès refusé".into());
            }
            self.value = Some(command.to_owned());
            Ok(())
        }
        fn remove(&mut self) -> Result<(), String> {
            if self.read_only {
                return Err("Accès refusé".into());
            }
            self.value = None;
            Ok(())
        }
    }

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Program Files\Micro Studio\micro-studio.exe")
    }

    #[test]
    fn command_quotes_the_path_and_starts_minimized() {
        assert_eq!(command_for(&exe()), r#""C:\Program Files\Micro Studio\micro-studio.exe" --minimized"#);
        assert!(started_minimized(["micro-studio.exe".into(), "--minimized".into()]));
        assert!(!started_minimized(["micro-studio.exe".into()]));
    }

    #[test]
    fn enable_then_disable() {
        let mut k = FakeKey::default();
        assert!(!is_enabled(&k));
        set_enabled(&mut k, &exe(), true).expect("activation");
        assert!(is_enabled(&k));
        assert_eq!(k.value.as_deref(), Some(command_for(&exe()).as_str()));
        set_enabled(&mut k, &exe(), false).expect("désactivation");
        assert!(!is_enabled(&k));
        // Désactiver une entrée absente n'est pas une erreur.
        set_enabled(&mut k, &exe(), false).expect("déjà désactivé");
    }

    #[test]
    fn refresh_fixes_a_moved_program_only_when_enabled() {
        let mut k = FakeKey::default();
        assert_eq!(refresh(&mut k, &exe()), Ok(false), "désactivé : rien à écrire");
        assert!(k.value.is_none());
        k.value = Some(r#""D:\ancien\micro-studio.exe" --minimized"#.into());
        assert_eq!(refresh(&mut k, &exe()), Ok(true));
        assert_eq!(k.value, Some(command_for(&exe())));
        assert_eq!(refresh(&mut k, &exe()), Ok(false), "déjà à jour");
    }

    #[test]
    fn errors_are_reported_not_hidden() {
        let mut k = FakeKey { read_only: true, ..FakeKey::default() };
        assert!(set_enabled(&mut k, &exe(), true).is_err());
        k.value = Some("autre".into());
        assert!(set_enabled(&mut k, &exe(), false).is_err());
        assert!(refresh(&mut k, &exe()).is_err());
    }

    #[test]
    fn parses_reg_query_output() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\r\n    Micro Studio    REG_SZ    \"C:\\Program Files\\Micro Studio\\micro-studio.exe\" --minimized\r\n\r\n";
        assert_eq!(parse_query(out), Some(command_for(&exe())));
        assert_eq!(parse_query("    Autre    REG_SZ    x\r\n"), None);
        assert_eq!(parse_query(""), None);
    }
}
