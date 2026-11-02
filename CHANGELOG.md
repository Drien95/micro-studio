# Journal des modifications

Toutes les évolutions notables du projet sont consignées ici.

Le format suit [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) et le
projet respecte le [versionnage sémantique](https://semver.org/lang/fr/)
(`MAJEUR.MINEUR.CORRECTIF`).

## [Non publié]

### Ajouté
- Plan de test (`docs/TESTS.md`, ISO/IEC/IEEE 29119 et niveaux ISTQB) et 98
  tests automatiques : unitaires dans chaque module, intégration, système
  (interface pilotée avec `egui_kittest`), régression sonore (empreintes de
  référence par préréglage), propriétés (`proptest`), absence d'allocation
  dans le traitement temps réel.
- Tant que le son est arrêté, la liste des périphériques est relue toutes les
  2,5 s : VB-Cable installé pendant que le logiciel tourne est détecté et le
  son démarre tout seul.
- CI locale : `scripts/ci.ps1` (Windows) et `scripts/ci.sh`, mêmes
  vérifications que GitHub en moins d'une minute, et hook `pre-push`
  optionnel (`.githooks/`).
- Seuil de couverture bloquant en CI : 85 % des lignes (88,6 % mesurés).
- Trait `AudioBackend` et `InputProcessor` / `OutputProcessor` : le moteur
  audio et l'interface sont testables sans carte son.
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

### Sécurité
- ringbuf 0.4 → 0.6 : faille RUSTSEC-2026-0293 (double libération dans
  `Consumer::skip`), signalée par cargo-deny. Non exploitable ici (échantillons
  `f32` sans destructeur), corrigée par principe.

### Corrigé
- Les libellés « Micro » et « Sortie » s'affichaient après leur liste, ce qui
  laissait croire que « VB-Cable » était le micro ; ils sont maintenant avant.
- Le de-esser n'atténuait les sifflantes que d'environ 0,7 dB au lieu de la
  réduction demandée (6 dB par défaut), à cause du déphasage du filtre de
  séparation.
- Une sauvegarde des réglages qui échoue est maintenant signalée dans
  l'interface ; l'écriture est atomique (fichier temporaire puis renommage).
- Symboles impossibles à afficher avec les polices par défaut (carrés vides)
  retirés de l'interface ; curseurs de l'égaliseur nommés pour les lecteurs
  d'écran.
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
