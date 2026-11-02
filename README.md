# Micro Studio

[![CI](https://github.com/Drien95/micro-studio/actions/workflows/ci.yml/badge.svg)](https://github.com/Drien95/micro-studio/actions/workflows/ci.yml)
[![Windows](https://github.com/Drien95/micro-studio/actions/workflows/windows.yml/badge.svg)](https://github.com/Drien95/micro-studio/actions/workflows/windows.yml)

Logiciel Windows qui améliore la qualité de ton micro en temps réel, avec un
**calibrage automatique** qui trouve les meilleurs réglages pour ton micro, ta
pièce et ta voix. Le son traité est envoyé vers un micro virtuel que Discord,
OBS, Teams ou tes jeux peuvent utiliser.

## La chaîne de traitement

Dans l'ordre utilisé par les ingés son :

| # | Effet | Rôle |
|---|---|---|
| 1 | Gain d'entrée | Amène la voix au bon niveau |
| 2 | Filtre passe-haut (24 dB/oct.) | Coupe grondements, vibrations, « boum » des plosives |
| 3 | Anti-bourdonnement (6 coupe-bandes) | Retire le « zzzz » du secteur ou de l'USB |
| 4 | Noise gate | Coupe le micro quand tu ne parles pas |
| 5 | Suppression de bruit IA (RNNoise ou DeepFilterNet3) | Retire ventilateur, clavier, souffle |
| 6 | Porte vocale (option) | Coupe ce que l'IA n'entend pas comme de la voix |
| 7 | Anti-réverbération (option) | Retire l'écho de la pièce |
| 8 | De-esser | Adoucit les « s » sifflants |
| 9 | Égaliseur 4 bandes | Corps, son « carton », présence, air |
| 10 | Égaliseur dynamique (option) | Adoucit le son « carton » et la dureté quand ils débordent |
| 11 | Compresseur multibande (option) | Graves, médiums et aigus tenus séparément |
| 12 | Compresseur | Volume régulier |
| 13 | Limiteur (anticipation 1,5 ms) | Aucune saturation possible |

Latence totale : environ 10 ms de traitement + 20 ms de tampon.

## Le calibrage automatique

1. **8 s de silence** : mesure du bruit de fond (pièce + micro) et recherche des bourdonnements (secteur 50 Hz et harmoniques, sifflement USB).
2. **8 s de frappe au clavier, sans parler** : mesure du bruit des touches et de la souris.
3. **Lecture d'un texte** (15 à 40 s) : mesure du niveau, du timbre, des sifflantes et des variations de volume de ta voix.
4. **Proposition** : réglages déduits des mesures (gain, coupe-bandes anti-bourdonnement, seuil du gate au-dessus des touches, porte vocale, fréquence du de-esser, corrections d'EQ par rapport au spectre moyen de la parole, ratio du compresseur).
5. **Tournoi** : 54 variantes avec RNNoise, puis les 6 meilleures repassent avec DeepFilterNet. Les 60 variantes sont rendues sur ton enregistrement et notées (voix au-dessus du bruit et du clavier, sonie en LUFS selon UIT-R BS.1770 avec une cible de -18 LUFS, régularité du volume, mots coupés, saturation, équilibre tonal). Le meilleur réglage est ensuite affiné : chaque paramètre est retouché un peu plus haut puis un peu plus bas, à pas de plus en plus fins, tant que le score progresse. Enfin, les 3 meilleurs sont écoutés par DNSMOS P.835, un réseau de Microsoft qui prédit la note (1 à 5) que donneraient des auditeurs, et cette note les départage. La meilleure gagne et un rapport avant/après s'affiche.
6. **Écoute avant/après**, puis tu gardes ou tu annules.

## Installation

1. Télécharge `micro-studio.exe` depuis l'onglet **Releases** (ou **Actions** > dernier build > *micro-studio-windows*) et lance-le.
2. Si le micro virtuel manque, clique sur **Installer le micro virtuel (VB-Cable)** : Micro Studio télécharge [VB-Cable](https://vb-audio.com/Cable/) depuis le site officiel, vérifie sa signature et ouvre son installation (accepte la demande administrateur, puis clique sur *Install Driver*). Le son démarre tout seul dès que le câble apparaît ; sinon, redémarre le PC. Choisis ensuite ton micro en haut ; la sortie se met d'elle-même sur *CABLE Input*.
3. VB-CABLE est un donationware de VB-Audio ([www.vb-cable.com](https://www.vb-cable.com)) : toutes les participations sont les bienvenues.
4. Lance le **Calibrage automatique**.
5. Dans Discord, OBS ou ton jeu, choisis **CABLE Output** comme micro.

Pour t'entendre : coche **M'écouter dans le casque** sous les vumètres et choisis ton casque.

Les réglages sont sauvegardés dans `%APPDATA%\micro-studio\settings.json` et rechargés au démarrage.

## Compiler soi-même

Rust 1.95 au minimum est nécessaire (egui/eframe 0.37). La version utilisée
par le projet est figée dans `rust-toolchain.toml` et s'installe toute seule
avec rustup.

```
# Rust : https://rustup.rs
cargo run --release
cargo test --release
```

## Qualité

Le code suit le format officiel Rust, passe clippy en mode strict sans
avertissement et est testé sous Windows, Linux et macOS à chaque modification.
Les normes appliquées (ISO/IEC 25010, règles NASA « Power of 10 », SemVer…) et
ce qui reste à faire sont détaillés dans [docs/QUALITE.md](docs/QUALITE.md),
la stratégie de test (ISO/IEC/IEEE 29119) dans [docs/TESTS.md](docs/TESTS.md).
Avant d'envoyer une modification : `.\scripts\ci.ps1` lance en local les
mêmes vérifications que GitHub. Pour contribuer : [CONTRIBUTING.md](CONTRIBUTING.md). Historique des versions :
[CHANGELOG.md](CHANGELOG.md).

## Feuille de route

- [x] Chaîne complète temps réel, interface, préréglages
- [x] Calibrage automatique avec tournoi de réglages
- [ ] Note de qualité par IA (DNSMOS, modèle ONNX de Microsoft) dans le tournoi
- [x] DeepFilterNet en option « qualité maximale » (choisi par le calibrage s'il fait mieux)
- [x] Notre propre moteur d'inférence DeepFilterNet, sans allocation, vérifié contre l'officiel
- [x] Note de qualité perçue DNSMOS P.835 dans le calibrage (exécution maison)
- [x] Compresseur multibande (3 bandes, Linkwitz-Riley)
- [x] Égaliseur dynamique (2 bandes)
- [x] Anti-réverbération
- [x] Écoute de contrôle dans le casque depuis l'application (« M'écouter dans le casque »)
- [ ] Icône dans la barre des tâches, lancement au démarrage de Windows
- [x] Installation de VB-Cable en un clic depuis l'application
- [ ] Micro virtuel intégré (pilote signé, prototype dans `driver/`) pour se passer de VB-Cable

## Licence

MIT. Utilise [nnnoiseless](https://github.com/jneem/nnnoiseless) (port Rust de RNNoise, BSD) et [DeepFilterNet3](https://github.com/Rikorose/DeepFilterNet) de Hendrik Schröter : seuls ses poids entraînés sont repris (MIT ou Apache-2.0, voir `models/DeepFilterNet3/`), le moteur qui les exécute est le nôtre. La note de qualité DNSMOS P.835 utilise les poids de Microsoft ([DNS-Challenge](https://github.com/microsoft/DNS-Challenge), CC BY 4.0, voir `models/DNSMOS/`), exécutés par notre propre code.
