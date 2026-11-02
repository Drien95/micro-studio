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
| 3 | Noise gate | Coupe le micro quand tu ne parles pas |
| 4 | Suppression de bruit IA (RNNoise) | Retire ventilateur, clavier, souffle |
| 5 | De-esser | Adoucit les « s » sifflants |
| 6 | Égaliseur 4 bandes | Corps, son « carton », présence, air |
| 7 | Compresseur | Volume régulier |
| 8 | Limiteur (anticipation 1,5 ms) | Aucune saturation possible |

Latence totale : environ 10 ms de traitement + 20 ms de tampon.

## Le calibrage automatique

1. **8 s de silence** : mesure du bruit de fond (pièce + micro).
2. **Lecture d'un texte** (15 à 40 s) : mesure du niveau, du timbre, des sifflantes et des variations de volume de ta voix.
3. **Proposition** : réglages déduits des mesures (gain, seuil du gate, fréquence du de-esser, corrections d'EQ par rapport au spectre moyen de la parole, ratio du compresseur).
4. **Tournoi** : 36 variantes sont rendues sur ton enregistrement et notées (réduction du bruit, régularité du volume, mots coupés, saturation, équilibre tonal). La meilleure gagne.
5. **Écoute avant/après**, puis tu gardes ou tu annules.

## Installation

1. Installe [VB-Cable](https://vb-audio.com/Cable/) (gratuit) et redémarre le PC. C'est lui qui crée le micro virtuel.
2. Télécharge `micro-studio.exe` depuis l'onglet **Releases** (ou **Actions** > dernier build > *micro-studio-windows*) et lance-le.
3. Choisis ton micro en haut ; la sortie se met d'elle-même sur *CABLE Input*.
4. Lance le **Calibrage automatique**.
5. Dans Discord, OBS ou ton jeu, choisis **CABLE Output** comme micro.

Pour t'entendre : Paramètres Windows > Son > *CABLE Output* > Propriétés > Écouter > cocher « Écouter ce périphérique ».

Les réglages sont sauvegardés dans `%APPDATA%\micro-studio\settings.json` et rechargés au démarrage.

## Compiler soi-même

```
# Rust : https://rustup.rs
cargo run --release
cargo test --release
```

## Qualité

Le code suit le format officiel Rust, passe clippy en mode strict sans
avertissement et est testé sous Windows, Linux et macOS à chaque modification.
Les normes appliquées (ISO/IEC 25010, règles NASA « Power of 10 », SemVer…) et
ce qui reste à faire sont détaillés dans [docs/QUALITE.md](docs/QUALITE.md).
Pour contribuer : [CONTRIBUTING.md](CONTRIBUTING.md). Historique des versions :
[CHANGELOG.md](CHANGELOG.md).

## Feuille de route

- [x] Chaîne complète temps réel, interface, préréglages
- [x] Calibrage automatique avec tournoi de réglages
- [ ] Note de qualité par IA (DNSMOS, modèle ONNX de Microsoft) dans le tournoi
- [ ] DeepFilterNet en option « qualité maximale »
- [ ] Écoute de contrôle dans le casque depuis l'application
- [ ] Icône dans la barre des tâches, lancement au démarrage de Windows
- [ ] Micro virtuel intégré (driver signé) pour se passer de VB-Cable

## Licence

MIT. Utilise [nnnoiseless](https://github.com/jneem/nnnoiseless) (port Rust de RNNoise, BSD).
