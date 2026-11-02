# Journal des modifications

Toutes les évolutions notables du projet sont consignées ici.

Le format suit [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) et le
projet respecte le [versionnage sémantique](https://semver.org/lang/fr/)
(`MAJEUR.MINEUR.CORRECTIF`).

## [Non publié]

### Ajouté
- Tests d'intégration (`tests/`) : absence d'allocation dans le traitement
  temps réel, robustesse face aux entrées et réglages extrêmes, compatibilité
  des fichiers de réglages, calibrage déterministe et sans plantage.
- CI qualité : format, clippy en mode strict, tests sous Windows, Linux et
  macOS, documentation, version minimale de Rust, sécurité et licences des
  dépendances (cargo-deny), mesure de couverture.
- Dependabot, modèle de pull request, `CONTRIBUTING.md`, `docs/QUALITE.md`.
- Publication : le tag doit correspondre à la version de `Cargo.toml` et les
  notes de version viennent de ce fichier.

### Modifié
- Le code est séparé en bibliothèque (`micro_studio`) et binaire pour pouvoir
  être testé sans carte son.
- Code formaté avec rustfmt, zéro avertissement clippy (pedantic).

### Corrigé
- Un échantillon invalide (NaN ou infini) envoyé par le pilote audio faisait
  planter le débruitage ; il est désormais remplacé par du silence.
- Un facteur Q nul ou invalide dans un fichier de réglages rendait les filtres
  instables.
- Le calibrage ne peut plus planter sur un score non numérique.
- Une panique dans un thread ne bloque plus l'accès aux réglages (Mutex
  empoisonné).

## [0.1.0] - 2026-11-02

### Ajouté
- Chaîne de traitement temps réel : gain, passe-haut, noise gate, RNNoise,
  de-esser, égaliseur 4 bandes, compresseur, limiteur.
- Interface egui en français, vumètres, préréglages, sauvegarde des réglages.
- Calibrage automatique avec tournoi de 36 variantes.
- Sortie vers VB-Cable, build Windows automatique.

[Non publié]: https://github.com/Drien95/micro-studio/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Drien95/micro-studio/releases/tag/v0.1.0
