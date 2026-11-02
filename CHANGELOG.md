# Journal des modifications

Toutes les évolutions notables du projet sont consignées ici.

Le format suit [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) et le
projet respecte le [versionnage sémantique](https://semver.org/lang/fr/)
(`MAJEUR.MINEUR.CORRECTIF`).

## [Non publié]

### Ajouté
- Moteur de suppression de bruit « premium » DeepFilterNet3 (`dsp::deepfilter`,
  via `deep-filter-rt`, modèle embarqué vérifié identique à l'original par
  SHA-256). Au choix avec RNNoise dans « Suppression de bruit ». En temps réel
  il tourne dans son propre fil (canaux bornés, aucune allocation côté audio),
  pour 40 ms de retard au total. Le calibrage fait repasser ses 6 meilleures
  variantes avec DeepFilterNet et garde le meilleur des deux moteurs ; les
  rendus sont réalignés sur l'original pour ne pas pénaliser le retard.
- Calibrage : nouvelle étape « tape au clavier sans parler » (8 s). Les
  touches sont mesurées, la porte de bruit est placée au-dessus de leurs
  crêtes quand c'est possible, et le tournoi note maintenant l'écart entre la
  voix et le clavier.
- Porte vocale : coupe ce que le réseau de neurones (RNNoise) n'entend pas
  comme de la voix, même fort (clavier, souris). Elle exige 30 ms de voix
  continue avant de s'ouvrir et retarde le son d'autant (20 ms) pour ne pas
  couper le début des mots. Le calibrage choisit de l'activer ou non.
- Anti-bourdonnement : le calibrage cherche les raies continues dans le
  silence (secteur 50 Hz et harmoniques, sifflement USB, FFT de 16 384 points)
  et place jusqu'à 6 filtres coupe-bande étroits.
- Mesure de sonie intégrée en LUFS (UIT-R BS.1770, module `dsp::loudness`) :
  le calibrage vise -18 LUFS au lieu d'un niveau RMS maison.
- Rapport de calibrage « sans traitement / avec tes réglages » : voix au-dessus
  du bruit et du clavier, volume en LUFS, régularité, mots coupés, équilibre.
- Retour casque « M'écouter dans le casque » (module `monitor`) : rejoue le
  son tel que le reçoivent Discord ou OBS (capté sur *CABLE Output*), avec choix
  du casque et volume. Le câble virtuel n'est jamais utilisé comme sortie du
  retour, pour éviter une boucle.
- Bouton « Installer le micro virtuel (VB-Cable) » : téléchargement du paquet
  officiel de VB-Audio, vérification de sa signature Authenticode, lancement
  de l'installation en administrateur, puis démarrage automatique du son dès
  que *CABLE Input* apparaît (module `vbcable`). Mention « donationware »
  exigée par VB-Audio affichée à côté.
- Prototype de pilote de micro virtuel « Micro Studio Cable » (`driver/`,
  mode test uniquement, voir `driver/README.md`).
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
- Après l'installation de VB-Cable, Windows prend souvent *CABLE Output* comme
  micro par défaut : Micro Studio s'écoutait alors lui-même en boucle. La
  sortie du câble n'est plus proposée comme micro et est remplacée au
  démarrage par le premier vrai micro.
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
