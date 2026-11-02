//! Installation de VB-Cable « en un clic » depuis Micro Studio.
//!
//! VB-Cable (VB-Audio, donationware) crée le couple *CABLE Input* (sortie où
//! Micro Studio envoie la voix traitée) / *CABLE Output* (micro à choisir dans
//! Discord, OBS…). VB-Audio autorise sa distribution avec une application à
//! condition de le présenter comme un produit VB-Audio et de rappeler qu'il
//! est un donationware : voir [`CREDIT`] et
//! <https://vb-audio.com/Services/licensing.htm>.
//!
//! Déroulé de [`install`] :
//! 1. téléchargement du paquet officiel et décompression dans un dossier
//!    temporaire (PowerShell) ;
//! 2. choix du programme d'installation adapté au processeur ([`pick_setup`]) ;
//! 3. vérification de la signature Authenticode : il doit être signé et
//!    valide, par VB-Audio ([`is_trusted_signature`]) ;
//! 4. lancement en administrateur (fenêtre UAC de Windows), l'utilisateur
//!    clique sur « Install Driver ».
//!
//! Toute la logique passe par le trait [`Shell`] : les tests remplacent
//! PowerShell par un faux et vérifient chaque cas d'erreur sans réseau.

use std::fmt;
use std::path::{Path, PathBuf};

/// Paquet officiel (page <https://vb-audio.com/Cable/>, version d'octobre 2024).
pub const PACKAGE_URL: &str = "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip";

/// Mention exigée par VB-Audio quand VB-Cable est proposé par une autre application.
pub const CREDIT: &str =
    "VB-CABLE est un donationware de VB-Audio (www.vb-cable.com) : toutes les participations sont les bienvenues.";

/// Page où l'utilisateur peut soutenir VB-Audio.
pub const ORIGIN_URL: &str = "https://www.vb-cable.com";

/// Architecture du processeur, pour choisir le bon programme d'installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86,
    X64,
    Arm64,
}

impl Arch {
    /// Architecture pour laquelle Micro Studio a été compilé.
    #[must_use]
    pub fn current() -> Self {
        match std::env::consts::ARCH {
            "x86" => Self::X86,
            "aarch64" => Self::Arm64,
            _ => Self::X64,
        }
    }
}

/// Étapes affichées pendant l'installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Downloading,
    Verifying,
    /// Le programme de VB-Audio est ouvert : l'utilisateur doit cliquer sur
    /// « Install Driver ».
    WaitingForSetup,
}

impl Step {
    /// Texte pour l'interface.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Downloading => "Téléchargement de VB-Cable…",
            Self::Verifying => "Vérification de la signature de VB-Audio…",
            Self::WaitingForSetup => "Dans la fenêtre de VB-Audio, clique sur « Install Driver », puis ferme-la.",
        }
    }
}

/// Résultat d'une tentative d'installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Le programme de VB-Audio s'est terminé normalement.
    Finished,
    /// L'utilisateur a refusé l'autorisation administrateur.
    Cancelled,
    /// Échec, avec un message lisible.
    Failed(String),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Finished => f.write_str("Installation de VB-Cable terminée."),
            Self::Cancelled => f.write_str("Installation annulée : l'autorisation administrateur a été refusée."),
            Self::Failed(msg) => write!(f, "Installation de VB-Cable impossible : {msg}"),
        }
    }
}

/// Accès au système, remplacé par un faux dans les tests.
pub trait Shell {
    /// Exécute un script PowerShell ; `Ok(sortie standard)` si le code de
    /// sortie vaut 0, sinon `Err((code, message))`.
    ///
    /// # Errors
    /// Code de sortie non nul ou PowerShell introuvable.
    fn run(&self, script: &str) -> Result<String, (i32, String)>;

    /// Noms des fichiers d'un dossier (récursivement, chemins relatifs).
    ///
    /// # Errors
    /// Dossier illisible.
    fn list_files(&self, dir: &Path) -> std::io::Result<Vec<String>>;
}

/// Code de sortie du script de lancement quand l'UAC est refusée.
const EXIT_CANCELLED: i32 = 3;

/// Installe VB-Cable ; `progress` est appelé à chaque nouvelle étape.
pub fn install(shell: &dyn Shell, work_dir: &Path, arch: Arch, progress: &mut dyn FnMut(Step)) -> Outcome {
    progress(Step::Downloading);
    if let Err((_, msg)) = shell.run(&download_script(PACKAGE_URL, work_dir)) {
        return Outcome::Failed(format!("téléchargement échoué ({}).", first_line(&msg)));
    }

    let files = match shell.list_files(work_dir) {
        Ok(files) => files,
        Err(e) => return Outcome::Failed(format!("dossier temporaire illisible ({e}).")),
    };
    let Some(setup) = pick_setup(&files, arch) else {
        return Outcome::Failed("programme d'installation introuvable dans le paquet de VB-Audio.".into());
    };
    let setup = work_dir.join(setup);

    progress(Step::Verifying);
    match shell.run(&signature_script(&setup)) {
        Ok(out) if is_trusted_signature(&out) => {}
        Ok(out) => {
            return Outcome::Failed(format!(
                "signature non reconnue ({}), installation bloquée par sécurité.",
                first_line(&out)
            ));
        }
        Err((_, msg)) => return Outcome::Failed(format!("vérification impossible ({}).", first_line(&msg))),
    }

    progress(Step::WaitingForSetup);
    match shell.run(&launch_script(&setup)) {
        Ok(_) => Outcome::Finished,
        Err((EXIT_CANCELLED, _)) => Outcome::Cancelled,
        Err((_, msg)) => Outcome::Failed(format!("le programme de VB-Audio a échoué ({}).", first_line(&msg))),
    }
}

/// Choisit le programme d'installation adapté parmi les fichiers du paquet.
///
/// Le paquet contient `VBCABLE_Setup.exe` (32 bits), `VBCABLE_Setup_x64.exe`
/// et, depuis la version 45, une variante ARM64 : on cherche d'abord la
/// variante exacte, puis la version 32 bits en dernier recours sur x86.
#[must_use]
pub fn pick_setup(files: &[String], arch: Arch) -> Option<String> {
    let setups: Vec<&String> = files
        .iter()
        .filter(|f| {
            let name = file_name(f).to_ascii_lowercase();
            name.starts_with("vbcable_setup")
                && Path::new(&name).extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        })
        .collect();
    let has = |suffix: &str| {
        setups
            .iter()
            .find(|f| file_name(f).to_ascii_lowercase().trim_end_matches(".exe").ends_with(suffix))
            .map(|f| (*f).clone())
    };
    match arch {
        Arch::X64 => has("_x64"),
        Arch::Arm64 => has("_arm64").or_else(|| has("_a64")),
        Arch::X86 => {
            setups.iter().find(|f| file_name(f).eq_ignore_ascii_case("vbcable_setup.exe")).map(|f| (*f).clone())
        }
    }
}

/// Vrai si la sortie de [`signature_script`] décrit une signature valide de VB-Audio.
///
/// Format attendu : `Statut|Sujet du certificat`, par exemple
/// `Valid|CN=Vincent Burel, O=Vincent Burel, L=…`.
#[must_use]
pub fn is_trusted_signature(output: &str) -> bool {
    let line = first_line(output);
    let Some((status, subject)) = line.split_once('|') else {
        return false;
    };
    let subject = subject.to_ascii_lowercase();
    status.trim() == "Valid" && (subject.contains("vincent burel") || subject.contains("vb-audio"))
}

/// Dossier de travail par défaut (`%TEMP%\micro-studio-vbcable`).
#[must_use]
pub fn default_work_dir() -> PathBuf {
    std::env::temp_dir().join("micro-studio-vbcable")
}

/// Met une chaîne entre apostrophes pour PowerShell (`'` devient `''`).
#[must_use]
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn download_script(url: &str, dir: &Path) -> String {
    let dir = ps_quote(&dir.display().to_string());
    format!(
        "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'\n\
         if (Test-Path -LiteralPath {dir}) {{ Remove-Item -LiteralPath {dir} -Recurse -Force }}\n\
         New-Item -ItemType Directory -Path {dir} | Out-Null\n\
         $zip = Join-Path {dir} 'vbcable.zip'\n\
         Invoke-WebRequest -UseBasicParsing -Uri {url} -OutFile $zip\n\
         Expand-Archive -LiteralPath $zip -DestinationPath {dir} -Force\n\
         Remove-Item -LiteralPath $zip\n",
        url = ps_quote(url),
    )
}

fn signature_script(setup: &Path) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $s = Get-AuthenticodeSignature -LiteralPath {}\n\
         $subject = if ($s.SignerCertificate) {{ $s.SignerCertificate.Subject }} else {{ '' }}\n\
         Write-Output (\"{{0}}|{{1}}\" -f $s.Status, $subject)\n",
        ps_quote(&setup.display().to_string())
    )
}

fn launch_script(setup: &Path) -> String {
    let setup = ps_quote(&setup.display().to_string());
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         try {{\n\
         \x20 $p = Start-Process -FilePath {setup} -WorkingDirectory (Split-Path -LiteralPath {setup}) -Verb RunAs -Wait -PassThru\n\
         }} catch {{ exit {EXIT_CANCELLED} }}\n\
         # Le code de retour du programme de VB-Audio n'est pas documenté :\n\
         # le succès se juge à l'apparition de CABLE Input (voir l'interface).\n\
         exit 0\n"
    )
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn first_line(s: &str) -> &str {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("sans détail")
}

/// Encode un script pour `powershell -EncodedCommand` (UTF-16LE puis Base64).
#[must_use]
pub fn encode_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(TABLE[((n >> (18 - 6 * i)) & 0x3F) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Le vrai système : Windows PowerShell 5.1, fourni avec Windows 10/11.
#[derive(Debug, Default, Clone, Copy)]
pub struct PowerShell;

impl Shell for PowerShell {
    fn run(&self, script: &str) -> Result<String, (i32, String)> {
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"])
            .arg(encode_command(script));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd.output().map_err(|e| (-1, format!("PowerShell introuvable : {e}")))?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if out.status.success() {
            Ok(stdout)
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            Err((out.status.code().unwrap_or(-1), if stderr.trim().is_empty() { stdout } else { stderr }))
        }
    }

    fn list_files(&self, dir: &Path) -> std::io::Result<Vec<String>> {
        let mut files = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        // Le paquet compte une dizaine de fichiers sur deux niveaux : la borne
        // évite toute boucle sans fin (lien symbolique cyclique).
        let mut budget = 1_000_usize;
        while let Some(current) = pending.pop() {
            for entry in std::fs::read_dir(&current)? {
                budget = budget.saturating_sub(1);
                if budget == 0 {
                    return Ok(files);
                }
                let path = entry?.path();
                if path.is_dir() {
                    pending.push(path);
                } else if let Ok(rel) = path.strip_prefix(dir) {
                    files.push(rel.display().to_string());
                }
            }
        }
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    const PACK45: &[&str] = &[
        "readme.txt",
        "VBCABLE_ControlPanel.exe",
        "VBCABLE_Setup.exe",
        "VBCABLE_Setup_x64.exe",
        "VBCABLE_Setup_arm64.exe",
        "vbMmeCable64_win10.inf",
    ];

    #[test]
    fn picks_the_setup_matching_the_processor() {
        let files = names(PACK45);
        assert_eq!(pick_setup(&files, Arch::X64).as_deref(), Some("VBCABLE_Setup_x64.exe"));
        assert_eq!(pick_setup(&files, Arch::Arm64).as_deref(), Some("VBCABLE_Setup_arm64.exe"));
        assert_eq!(pick_setup(&files, Arch::X86).as_deref(), Some("VBCABLE_Setup.exe"));
    }

    #[test]
    fn pick_setup_handles_subfolders_case_and_absence() {
        let files = names(&["Pack\\vbcable_setup_X64.EXE", "Pack/VBCABLE_Setup_a64.exe"]);
        assert_eq!(pick_setup(&files, Arch::X64).as_deref(), Some("Pack\\vbcable_setup_X64.EXE"));
        assert_eq!(pick_setup(&files, Arch::Arm64).as_deref(), Some("Pack/VBCABLE_Setup_a64.exe"));
        assert_eq!(pick_setup(&files, Arch::X86), None);
        assert_eq!(pick_setup(&names(&["VBCABLE_Setup_x64.zip", "setup_x64.exe"]), Arch::X64), None);
        assert_eq!(pick_setup(&[], Arch::X64), None);
    }

    #[test]
    fn only_a_valid_vb_audio_signature_is_trusted() {
        assert!(is_trusted_signature("Valid|CN=Vincent Burel, O=Vincent Burel, C=FR\r\n"));
        assert!(is_trusted_signature("\nValid|CN=VB-Audio Software"));
        assert!(!is_trusted_signature("NotSigned|"));
        assert!(!is_trusted_signature("HashMismatch|CN=Vincent Burel"));
        assert!(!is_trusted_signature("Valid|CN=Someone Else"));
        assert!(!is_trusted_signature("Valid"));
        assert!(!is_trusted_signature(""));
    }

    #[test]
    fn quoting_and_encoding_are_correct() {
        assert_eq!(ps_quote(r"C:\Users\l'ami\x"), r"'C:\Users\l''ami\x'");
        // Vecteurs de la RFC 4648.
        for (input, expected) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
        // « dir » en UTF-16LE, forme attendue par -EncodedCommand.
        assert_eq!(encode_command("dir"), "ZABpAHIA");
    }

    #[test]
    fn scripts_target_the_right_files() {
        let dir = Path::new(r"C:\Temp\ms vb");
        let dl = download_script(PACKAGE_URL, dir);
        assert!(dl.contains(&ps_quote(PACKAGE_URL)) && dl.contains(r"'C:\Temp\ms vb'"));
        assert!(dl.contains("Expand-Archive"));
        let setup = dir.join("VBCABLE_Setup_x64.exe");
        assert!(signature_script(&setup).contains("Get-AuthenticodeSignature"));
        let launch = launch_script(&setup);
        assert!(launch.contains("-Verb RunAs") && launch.contains(&format!("exit {EXIT_CANCELLED}")));
    }

    #[test]
    fn texts_are_human_readable() {
        assert!(Step::WaitingForSetup.label().contains("Install Driver"));
        assert!(Step::Downloading.label().contains("Téléchargement"));
        assert!(Step::Verifying.label().contains("signature"));
        assert!(Outcome::Finished.to_string().contains("terminée"));
        assert!(Outcome::Cancelled.to_string().contains("annulée"));
        assert!(Outcome::Failed("x".into()).to_string().ends_with('x'));
        assert!(CREDIT.contains("www.vb-cable.com") && CREDIT.contains("donationware"));
        assert!(default_work_dir().ends_with("micro-studio-vbcable"));
        assert_eq!(first_line("  \n"), "sans détail");
        let _ = Arch::current();
    }

    /// Faux PowerShell : répond selon le script reçu et note les appels.
    struct FakeShell {
        download: Result<String, (i32, String)>,
        files: std::io::Result<Vec<String>>,
        signature: Result<String, (i32, String)>,
        launch: Result<String, (i32, String)>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl FakeShell {
        fn ok() -> Self {
            Self {
                download: Ok(String::new()),
                files: Ok(names(PACK45)),
                signature: Ok("Valid|CN=Vincent Burel".into()),
                launch: Ok(String::new()),
                calls: RefCell::default(),
            }
        }
    }

    impl Shell for FakeShell {
        fn run(&self, script: &str) -> Result<String, (i32, String)> {
            let (name, answer) = if script.contains("Invoke-WebRequest") {
                ("download", &self.download)
            } else if script.contains("Get-AuthenticodeSignature") {
                assert!(script.contains("VBCABLE_Setup_x64.exe"));
                ("signature", &self.signature)
            } else {
                assert!(script.contains("RunAs"));
                ("launch", &self.launch)
            };
            self.calls.borrow_mut().push(name);
            answer.clone()
        }

        fn list_files(&self, _: &Path) -> std::io::Result<Vec<String>> {
            match &self.files {
                Ok(f) => Ok(f.clone()),
                Err(e) => Err(std::io::Error::new(e.kind(), e.to_string())),
            }
        }
    }

    fn run(shell: &FakeShell) -> (Outcome, Vec<Step>) {
        let mut steps = Vec::new();
        let outcome = install(shell, Path::new("work"), Arch::X64, &mut |s| steps.push(s));
        (outcome, steps)
    }

    #[test]
    fn happy_path_runs_every_step_in_order() {
        let shell = FakeShell::ok();
        let (outcome, steps) = run(&shell);
        assert_eq!(outcome, Outcome::Finished);
        assert_eq!(steps, [Step::Downloading, Step::Verifying, Step::WaitingForSetup]);
        assert_eq!(*shell.calls.borrow(), ["download", "signature", "launch"]);
    }

    #[test]
    fn untrusted_signature_never_launches_the_setup() {
        let shell = FakeShell { signature: Ok("Valid|CN=Pirate".into()), ..FakeShell::ok() };
        let (outcome, _) = run(&shell);
        assert!(matches!(outcome, Outcome::Failed(ref m) if m.contains("signature non reconnue")));
        assert_eq!(*shell.calls.borrow(), ["download", "signature"]);
    }

    #[test]
    fn each_failure_is_reported() {
        let shell = FakeShell { download: Err((1, "Hôte inconnu\nligne 2".into())), ..FakeShell::ok() };
        assert!(
            matches!(run(&shell).0, Outcome::Failed(ref m) if m.contains("Hôte inconnu") && !m.contains("ligne 2"))
        );

        let shell = FakeShell { files: Err(std::io::Error::other("refusé")), ..FakeShell::ok() };
        assert!(matches!(run(&shell).0, Outcome::Failed(ref m) if m.contains("illisible")));

        let shell = FakeShell { files: Ok(names(&["readme.txt"])), ..FakeShell::ok() };
        assert!(matches!(run(&shell).0, Outcome::Failed(ref m) if m.contains("introuvable")));

        let shell = FakeShell { signature: Err((1, "boom".into())), ..FakeShell::ok() };
        assert!(matches!(run(&shell).0, Outcome::Failed(ref m) if m.contains("vérification impossible")));

        let shell = FakeShell { launch: Err((EXIT_CANCELLED, String::new())), ..FakeShell::ok() };
        assert_eq!(run(&shell).0, Outcome::Cancelled);

        let shell = FakeShell { launch: Err((5, "code 5".into())), ..FakeShell::ok() };
        assert!(matches!(run(&shell).0, Outcome::Failed(ref m) if m.contains("code 5")));
    }

    #[test]
    fn list_files_walks_subfolders() {
        let dir = std::env::temp_dir().join(format!("ms-vbcable-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).expect("dossier de test");
        std::fs::write(dir.join("a.txt"), b"a").expect("fichier de test");
        std::fs::write(dir.join("sub").join("VBCABLE_Setup_x64.exe"), b"b").expect("fichier de test");
        let mut files = PowerShell.list_files(&dir).expect("lecture");
        files.sort();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(files.len(), 2);
        assert_eq!(
            pick_setup(&files, Arch::X64).map(|f| file_name(&f).to_owned()).as_deref(),
            Some("VBCABLE_Setup_x64.exe")
        );
    }
}
