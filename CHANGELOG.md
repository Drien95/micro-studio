# Journal des modifications

Toutes les évolutions notables du projet sont consignées ici.

Le format suit [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/) et le
projet respecte le [versionnage sémantique](https://semver.org/lang/fr/)
(`MAJEUR.MINEUR.CORRECTIF`).

## [Non publié]

### Modifié
- Calibrage : le gain de sortie peut monter jusqu'à +12 dB (au lieu de +6)
  pour atteindre -18 LUFS avec un micro USB faible (constaté : -21,9 LUFS sur
  le Logitech) ; les mots coupés pèsent plus lourd (300 points au lieu de
  200) et la clarté de la voix DNSMOS (SIG) entre dans le départage final
  (5 points par point de note).
- Micro : les micros virtuels (Voicemod, VoiceMeeter, NVIDIA Broadcast,
  Steam, Krisp) ne sont plus pris par défaut, le vrai micro est choisi à
  leur place. Un micro virtuel choisi à la main est gardé, mais signalé, avec
  un bouton pour passer au vrai micro (le calibrage mesurerait sinon
  l'autre logiciel).
- Moteur DeepFilterNet : convolutions sans test de bornes dans la boucle
  interne et produits scalaires à 16 accumulateurs (environ 6 % plus rapide,
  sortie toujours identique à l'officiel à 1e-7 près). Le produit scalaire
  est partagé avec DNSMOS.

### Ajouté
- Anti-réverbération (Lebart et al., 2001) : la réverbération tardive est
  estimée bande par bande à partir de la puissance d'il y a 50 ms et de la
  durée d'écho de la pièce, puis retirée par gain spectral avec plancher
  (jusqu'à 15 dB). Transformée courte de 20 ms, reconstruction parfaite,
  10 ms de retard, sans allocation. Le calibrage essaie de l'allumer et
  affine l'intensité et la durée d'écho.
- Égaliseur dynamique à 2 bandes (« carton » vers 400 Hz, dureté vers
  3 kHz) : chaque bande ne baisse que de l'excès au-dessus de son seuil
  (ratio 3, 6 dB au plus), avec une réduction exacte au centre de la bande.
  Le calibrage essaie de l'allumer et affine ses seuils.
- Compresseur multibande à 3 bandes (graves, médiums, aigus), entre
  l'égaliseur et le compresseur : filtres de séparation Linkwitz-Riley
  d'ordre 4 en phase (courbe plate à 0,1 dB près quand rien n'est compressé),
  temps d'attaque et de relâchement adaptés à chaque bande. Réglable dans sa
  section ; le calibrage essaie de l'allumer et affine ses seuils et ratios.
- Calibrage : manche d'affinage après le tournoi (descente par coordonnées).
  Partant du meilleur réglage, chaque paramètre fin (seuil et relâchement de
  la porte, dosage du débruitage, de-esser, seuil et ratio du compresseur,
  passe-haut, intensité de l'égaliseur, porte vocale) est retouché dans les
  deux sens ; le pas est divisé par deux quand plus rien ne progresse
  (3 tours, jusqu'à 60 réglages de plus). Sur les enregistrements de test, le
  score du meilleur réglage gagne de 6 à 11 points.
- Note de qualité perçue DNSMOS P.835 (Microsoft, ICASSP 2022) dans le
  calibrage : la voix, le bruit de fond et l'ensemble sont notés de 1 à 5
  comme le feraient des auditeurs (recommandation ITU-T P.835). Les 3
  meilleurs réglages du tournoi sont départagés par cette note (10 points de
  score par point de note globale) et le rapport affiche les trois notes
  avant/après. Exécution écrite pour Micro Studio (`dnsmos`) : spectre par
  noyaux de STFT appris, 7 convolutions 3×3 réparties sur tous les cœurs,
  décimation 48 → 16 kHz par sinus cardinal fenêtré (Kaiser). Vérifiée contre
  onnxruntime (écart < 1e-3 sur les notes brutes). Seuls les poids viennent
  de Microsoft (`models/DNSMOS/`, CC BY 4.0, empreinte SHA-256 notée).
- Moteur de suppression de bruit « premium » DeepFilterNet3, au choix avec
  RNNoise dans « Suppression de bruit » (30 ms de retard). Le calibrage fait
  repasser ses 6 meilleures variantes avec DeepFilterNet et garde le meilleur
  des deux moteurs ; les rendus sont réalignés sur l'original pour ne pas
  pénaliser le retard.
- Moteur d'inférence DeepFilterNet3 écrit pour Micro Studio (`dsp::dfn`) :
  lecteur ONNX minimal (`dfn::onnx`), couches (convolutions causales, GRU,
  linéaires par groupes, convolutions transposées), STFT, caractéristiques ERB
  et filtre profond. Aucune allocation après le chargement : il tourne dans le
  callback audio. Vérifié contre le moteur officiel `libDF` (écart maximal
  7e-8 sur la sortie), un peu plus rapide (0,75 contre 0,83 ms par trame
  mesurés). Seuls les poids viennent du projet DeepFilterNet
  (`models/DeepFilterNet3/`, MIT ou Apache-2.0, empreintes SHA-256 notées).
  Le moteur officiel n'est plus qu'une dépendance de test.
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
