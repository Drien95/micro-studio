# Plan de test

Stratégie de test de Micro Studio, structurée selon la norme
**ISO/IEC/IEEE 29119** (partie 3 : documentation de test) et les **niveaux de
test ISTQB**. Dernière mise à jour : 2 novembre 2026.

## 1. Objectifs

- Aucune régression du son d'une version à l'autre.
- Aucun plantage ni craquement, quelles que soient les entrées (pilote audio,
  fichier de réglages, réglages extrêmes).
- Respect des contraintes temps réel : aucune allocation ni attente bloquante
  dans le thread audio.
- Chaque bug corrigé est couvert par un test qui l'aurait détecté.

## 2. Niveaux de test

| Niveau ISTQB | Où | Ce qui est vérifié | Lancement |
|---|---|---|---|
| **Unitaire** (composant) | `#[cfg(test)] mod tests` dans chaque fichier de `src/` | Chaque fonction isolément : réponse en fréquence des filtres, courbe du compresseur, hystérésis du gate, rapport du rééchantillonneur, découpage en trames du moteur, amorçage et coupures de la sortie, analyse du calibrage, sauvegarde atomique… | `cargo test --lib` |
| **Intégration** (composants assemblés) | `tests/robustness.rs`, `tests/calibration.rs`, `tests/settings.rs`, `tests/realtime.rs` | La chaîne complète, le calibrage de bout en bout, la compatibilité des fichiers de réglages, l'absence d'allocation en temps réel | `cargo test --tests` |
| **Système** (application entière) | `tests/ui.rs` | L'application pilotée comme un utilisateur (clics, textes affichés) avec un faux périphérique audio : démarrage automatique sur VB-Cable, détection de VB-Cable installé en cours de route, assistant de calibrage, panne de micro, préréglages | `cargo test --test ui` |
| **Régression** | `tests/regression.rs` + `tests/golden/presets.json` | L'« empreinte » sonore de chaque préréglage (niveau par tranche de 100 ms) ne change pas à plus de 0,05 dB près | `cargo test --test regression` |
| **Propriétés** (aléatoire) | `tests/properties.rs` | Des centaines de réglages et signaux aléatoires : sortie finie, plafond respecté, déterminisme, silence préservé, aller-retour JSON | `cargo test --test properties` |
| **Acceptation** | Manuel, voir §5 | Le son avec le vrai micro, sur le PC de l'utilisateur | Protocole ci-dessous |

## 3. Techniques de conception des tests (ISO/IEC/IEEE 29119-4)

| Technique | Exemples |
|---|---|
| Partitions d'équivalence | Voix faible / forte / saturée ; périphérique avec ou sans VB-Cable |
| Valeurs limites | Q = 0, fréquence négative ou infinie, ratio < 1, plafond > 0 dB, enregistrement vide ou de 10 échantillons, tampon plein ou vide |
| Transitions d'états | Gate ouvert → maintien → fermé ; sortie amorcée → coupure → réamorçage ; assistant Intro → Silence |
| Injection de fautes | Échantillons NaN/±∞, Mutex empoisonné, fichier JSON corrompu, dossier de sauvegarde impossible à écrire, panne du périphérique audio |
| Tests par propriétés | `proptest` (génération et réduction automatique des contre-exemples) |
| Oracle de référence (golden) | Empreintes sonores enregistrées pour chaque préréglage |

## 4. Isolation et doublures de test

Tout le code est testable sans carte son ni écran :

- **Moteur audio** : la logique des callbacks est dans `InputProcessor` et
  `OutputProcessor`, qui ne dépendent pas de cpal. Les tests les alimentent
  avec des tampons et un tampon circulaire réels.
- **Matériel** : l'interface ne parle au matériel que via le trait
  `AudioBackend`. `CpalBackend` est le vrai, les tests utilisent un faux
  (`FakeBackend` dans `tests/ui.rs`) qui peut simuler une panne.
- **Disque** : `App::with_backend(…, None)` n'écrit jamais de fichier ;
  `Settings::save_to` et `load_from` prennent un chemin explicite.
- **Interface** : `egui_kittest` exécute l'interface sans fenêtre et la lit
  via l'arbre d'accessibilité (ce qui vérifie au passage que chaque contrôle a
  un nom lisible par un lecteur d'écran et qu'aucun caractère n'est
  impossible à afficher).

Reste hors des tests automatiques : les quelques lignes qui ouvrent réellement
les flux cpal (`start`, `build_input`, `build_output`, `list_devices`) et
`main.rs`. Elles sont couvertes par le test d'acceptation.

## 5. Test d'acceptation (manuel, avant chaque version)

Sur Windows 11 avec le micro réel et VB-Cable :

1. Lancer `micro-studio.exe` : le statut passe à « Actif », la sortie est
   *CABLE Input*.
2. Parler : les vumètres bougent, la porte s'ouvre, aucun craquement pendant
   5 minutes (compteur de coupures à 0).
3. Débrancher le micro USB puis le rebrancher : message d'erreur, puis
   « Démarrer » relance sans redémarrer le logiciel.
4. Calibrage complet : résultat proposé en moins de 30 s, écoute avant/après.
5. Discord ou OBS avec *CABLE Output* comme micro : voix claire, sans écho.
6. Fermer et rouvrir : les réglages sont conservés.

## 6. Où tournent les tests

- **En local**, avant chaque envoi : `scripts/ci.ps1` (ou `ci.sh`), en moins
  d'une minute une fois compilé, éventuellement via le hook `pre-push`.
- **Sur GitHub**, à chaque push et pull request : la même chose sur Windows,
  Linux et macOS, plus la version minimale de Rust. C'est la référence.

## 7. Critères de sortie

Une pull request n'est fusionnée que si, dans la CI :

- tous les tests passent sous Windows, Linux et macOS ;
- clippy (pedantic) et rustfmt ne signalent rien ;
- la couverture de lignes reste au-dessus de 85 % (seuil fixé dans
  `.github/workflows/ci.yml` ; 88,6 % au 2 novembre 2026) ;
- `cargo deny` ne signale aucune faille connue ni licence interdite.

## 8. Ajouter un test

- Nouvelle fonction : test unitaire dans le même fichier.
- Bug : d'abord un test qui échoue, puis la correction.
- Changement voulu du son : `UPDATE_GOLDEN=1 cargo test --test regression`,
  puis expliquer le diff de `tests/golden/presets.json` dans la pull request.
