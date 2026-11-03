# Journal des modifications : pilote Micro Studio Cable

Format : [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/),
versions : [SemVer](https://semver.org/lang/fr/).

## [Non publié]

### Ajouté
- Pilote de micro virtuel basé sur l'exemple *Simple Audio Sample* de Microsoft :
  la sortie « Micro Studio » est renvoyée vers le micro « Micro Studio ».
- Cœur portable `core/ms_loopback.c` (tampon circulaire, conversions PCM
  16/24/32 bits, mono/stéréo, amorçage anti-craquements, latence bornée).
- Tests unitaires et différentiels (gcc, clang, MSVC, sanitizers, 100 % des lignes).
- Scripts d'installation/désinstallation en mode test.
- CI : compilation WDK et paquet test-signé en artefact.
