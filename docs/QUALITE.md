# Qualité logicielle : normes suivies et état du projet

Ce document dit, norme par norme, ce que Micro Studio applique, comment c'est
vérifié automatiquement, et ce qui reste à faire. Il est mis à jour à chaque
évolution notable (dernière revue : 2 novembre 2026).

Micro Studio n'est pas un logiciel critique (personne n'est en danger s'il
plante). On applique donc les normes **de façon proportionnée** : tout ce qui
améliore vraiment la fiabilité d'une application audio temps réel, sans la
lourdeur administrative des processus aérospatiaux.

## 1. ISO/IEC 25010 : modèle de qualité du produit

La norme internationale de référence pour juger la qualité d'un logiciel
(famille SQuaRE, ISO/IEC 25000). La révision 2023 renomme « portabilité » en
« flexibilité » ; on garde ici le terme courant.

| Caractéristique | Ce qui est fait | Vérification |
|---|---|---|
| **Fiabilité** : tolérance aux fautes | Échantillons NaN/infinis du pilote remplacés par du silence ; réglages corrompus bornés (Q, fréquences) ; Mutex empoisonné récupéré ; aucun `unwrap()` hors tests | `tests/robustness.rs`, lint `clippy::unwrap_used` |
| **Fiabilité** : maturité | 98 tests automatiques (88,6 % des lignes couvertes) (unitaires, intégration, système, régression, propriétés), plan de test dans [TESTS.md](TESTS.md) | CI sur chaque pull request, seuil de couverture |
| **Fiabilité** : disponibilité temps réel | Aucune allocation ni verrou bloquant dans le thread audio ; tampon de 20 ms avec rattrapage de dérive d'horloge | `tests/realtime.rs` (allocateur instrumenté) |
| **Maintenabilité** : modularité | Bibliothèque (`dsp`, `params`, `calibrate`, `settings`) séparée du moteur, de l'interface et du binaire | Structure `src/lib.rs` |
| **Maintenabilité** : testabilité | Tout est testable sans carte son ni écran : logique des callbacks audio séparée de cpal, matériel derrière le trait `AudioBackend`, interface testée avec `egui_kittest` | `cargo test`, couverture mesurée en CI |
| **Utilisabilité** : accessibilité | Chaque contrôle a un nom lisible par un lecteur d'écran ; aucun caractère non affichable | `tests/ui.rs` (vérification d'accessibilité d'egui_kittest) |
| **Maintenabilité** : analysabilité | Format unique (rustfmt), clippy strict sans avertissement, documentation générée | CI `fmt`, `clippy`, `doc` |
| **Maintenabilité** : modifiabilité | Réglages compatibles avec les versions passées et futures (`serde(default)`, champs inconnus ignorés) | `tests/settings.rs` |
| **Portabilité** : adaptabilité | Code multiplateforme (cpal) ; compilé et testé sous Windows, Linux et macOS | CI `test` (3 OS) |
| **Portabilité** : installabilité | Un seul `.exe`, sans installation ; publication automatique par tag | Workflow `windows.yml` |
| **Sécurité** | Pas de code `unsafe` ; dépendances vérifiées contre la base de failles RustSec ; licences contrôlées | Lint `unsafe_code`, CI `deny` |
| **Efficacité** | Profil release LTO ; dépendances optimisées même en debug | `Cargo.toml` |

## 2. Règles « Power of 10 » de la NASA (G. Holzmann, JPL, 2006)

Écrites pour le C embarqué des missions spatiales ; voici leur traduction en
Rust pour ce projet.

| # | Règle d'origine | Application ici | État |
|---|---|---|---|
| 1 | Flux de contrôle simple, pas de récursion | Aucune récursion, aucun `goto` possible en Rust | ✅ |
| 2 | Toutes les boucles ont une borne fixe | Boucles sur des trames de taille fixe (480 échantillons) ; rééchantillonneur borné par le rapport de fréquences | ✅ |
| 3 | Pas d'allocation dynamique après l'initialisation | Appliqué au thread audio, vérifié par test ; l'interface et le calibrage (hors temps réel) allouent normalement | ✅ (temps réel) |
| 4 | Fonctions courtes (≤ 60 lignes) | Limite clippy à 100 lignes ; une exception documentée (assistant de calibrage) | 🟡 |
| 5 | Au moins deux assertions par fonction | Remplacé par le typage fort de Rust et les tests de propriétés | 🟡 adapté |
| 6 | Données à la portée la plus réduite | Garanti par Rust (propriété, emprunts) | ✅ |
| 7 | Vérifier chaque valeur de retour et paramètre | `Result` partout, `unwrap` interdit, paramètres bornés | 🟡 voir §5 |
| 8 | Usage limité du préprocesseur | Aucune macro maison | ✅ |
| 9 | Usage limité des pointeurs | `unsafe` interdit | ✅ |
| 10 | Zéro avertissement, analyse statique quotidienne | clippy `pedantic` en `-D warnings` à chaque push ; Dependabot hebdomadaire | ✅ |

Les normes de **processus** de la NASA (NPR 7150.2, NASA-STD-8739.8) et les
normes automobiles (MISRA, ISO 26262) ou ferroviaires visent des logiciels dont
une panne met des vies en jeu ; leur équivalent Rust est le guide
*Safety-Critical Rust Coding Guidelines* (Ferrocene). Elles ne s'appliquent pas
à ce projet, mais leurs règles techniques utiles sont reprises ci-dessus.

## 3. Rust API Guidelines et conventions Rust

| Règle | État |
|---|---|
| Format officiel `rustfmt` (`rustfmt.toml` : lignes de 120 caractères) | ✅ |
| `clippy::pedantic` sans avertissement ; exceptions listées et justifiées dans `[lints]` de `Cargo.toml` | ✅ |
| Types publics implémentant `Debug` (C-DEBUG) | ✅ |
| Métadonnées du paquet complètes (C-METADATA) : licence, dépôt, mots-clés, `rust-version` | ✅ |
| Documentation de module (`//!`) et des types publics | 🟡 partielle |
| Édition 2024, version minimale de Rust déclarée et testée (1.95) | ✅ |

## 4. Versionnage et maintenance

| Pratique | Où |
|---|---|
| [SemVer 2.0](https://semver.org/lang/fr/) | `Cargo.toml`, tags `vX.Y.Z` |
| [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) | `CHANGELOG.md` |
| [Conventional Commits](https://www.conventionalcommits.org/fr/v1.0.0/) | `CONTRIBUTING.md` |
| Dépendances figées (`Cargo.lock`, `--locked` en CI) et mises à jour par Dependabot | `.github/dependabot.yml` |
| Revue par pull request avec liste de contrôle | `.github/pull_request_template.md` |
| Fins de ligne et encodage uniformes | `.gitattributes`, `.editorconfig` |

## 5. Ce qui reste à faire

Classé par utilité réelle :

1. **Test d'acceptation sur matériel** : l'ouverture réelle des flux cpal reste
   testée à la main, selon le protocole de [TESTS.md](TESTS.md) §5.
2. **Découper l'assistant de calibrage** (`ui.rs`) en fonctions plus courtes.
3. **Fuzzing** guidé par la couverture (`cargo-fuzz`, nécessite Rust nightly)
   en complément des tests par propriétés.
4. **Tests de performance** : mesurer le temps de traitement d'une trame
   (budget : 10 ms) et l'intégrer à la CI (`criterion`).
5. **Exécutable signé** (certificat de signature de code) pour éviter
   l'avertissement SmartScreen de Windows.
