# Micro Studio Cable : notre micro virtuel

Pilote Windows qui remplace VB-Cable. Il ajoute deux périphériques audio :

| Périphérique | Rôle |
|---|---|
| **Micro Studio** (sortie, « haut-parleurs ») | Micro Studio y envoie ta voix traitée |
| **Micro Studio** (entrée, « réseau de microphones ») | À choisir comme micro dans Discord, OBS, Teams, jeux… |

Tout ce qui entre dans la sortie ressort du micro, avec environ 10 ms de
latence ajoutée (réserve anti-craquements).

> **Statut : prototype.** Il n'est signé qu'avec un certificat de test, donc il
> ne s'installe qu'en « mode test » de Windows. Pour une version grand public,
> voir [Signature](#signature-pour-une-version-grand-public).

## Comment ça marche

```
Micro Studio (appli)            Pilote MicroStudioCable.sys (noyau)           Discord / OBS
  traitement de la voix  ──►  sortie virtuelle ──► boucle 200 ms ──► micro virtuel  ──►  capture
                              (ReadBytes)          core/ms_loopback.c   (WriteBytes)
```

- `MicroStudioCable/` : le pilote. Base : l'exemple officiel *Simple Audio
  Sample* de Microsoft (PortCls/WaveRT, licence MS-PL), importé tel quel dans
  un premier commit puis modifié : le générateur de tonalité du micro est
  remplacé par la lecture de la boucle, la sortie écrit dans la boucle, noms
  « Micro Studio ».
- `core/` : le cœur (tampon circulaire + conversions 16/24/32 bits, mono/stéréo),
  en C portable sans allocation ni récursion, pour être testé hors du noyau.
- `MicroStudioCable/Source/Utilities/cable.cpp` : la fine couche noyau
  (mémoire non paginée + verrou) autour du cœur.
- `tests/` : tests unitaires, de transitions d'état et différentiels.
- `scripts/` : installation et désinstallation en mode test.

Comportements garantis par les tests :
- le micro rend du **silence** tant que rien n'est joué (jamais de bruit) ;
- si la sortie va plus vite que le micro, l'audio **le plus ancien** est jeté :
  la latence ne dépasse jamais 200 ms ;
- après un trou, le micro attend 10 ms de réserve avant de reprendre
  (évite une série de clics) ;
- au démarrage du micro, l'audio périmé est jeté.

## Tests

```
make -C driver/tests test       # gcc/clang, -Werror, AddressSanitizer + UBSan
make -C driver/tests coverage   # échoue si la couverture des lignes < 100 %
```

La CI (`.github/workflows/driver.yml`) lance aussi les tests avec MSVC
(`/W4 /WX`), vérifie les scripts PowerShell et compile le pilote avec le WDK.
Le paquet test-signé est publié comme artefact **micro-studio-cable-x64**.

Non couvert automatiquement (exige une vraie machine Windows en mode test) :
chargement du pilote, apparition des périphériques, son de bout en bout.

## Installer le prototype (mode test)

⚠️ Le mode test exige de **désactiver le Secure Boot** dans le BIOS/UEFI, et
Windows affiche « Mode test » en bas à droite. Certains anti-triche de jeux
(Valorant, FACEIT…) refusent de fonctionner dans ce mode. À réserver aux essais.

1. Télécharge l'artefact **micro-studio-cable-x64** (onglet *Actions*, dernier
   build « Pilote micro virtuel ») et décompresse-le.
2. PowerShell **administrateur** dans ce dossier :
   ```
   Set-ExecutionPolicy -Scope Process Bypass
   .\Install-MicroStudioCable.ps1 -PackagePath .
   ```
   La première fois, le script active le mode test : redémarre puis relance-le.
3. Dans Micro Studio, choisis la sortie **Micro Studio** ; dans Discord, le
   micro **Micro Studio**.

Désinstaller : `.\Uninstall-MicroStudioCable.ps1`, puis, si tu veux quitter le
mode test, `bcdedit /set testsigning off` et redémarrer.

## Signature pour une version grand public

Windows 11 n'accepte un pilote noyau que s'il est signé par Microsoft :

1. **Certificat EV de signature de code** (environ 250 à 400 € par an, il en
   existe pour les entrepreneurs individuels, livré sur clé matérielle ou HSM
   dans le cloud).
2. **Compte « Windows Hardware Developer Program »** sur Partner Center, ouvert
   avec ce certificat.
3. **Signature d'attestation** : on envoie le paquet (CAB signé EV), Microsoft
   le contresigne, et il s'installe ensuite sur tout Windows 10/11 sans mode
   test. Pas de diffusion par Windows Update dans ce mode, ce qui ne nous
   gêne pas : l'installeur de Micro Studio l'installera lui-même.

Référence : <https://learn.microsoft.com/windows-hardware/drivers/dashboard/code-signing-attestation>

## Licences

Code Micro Studio : MIT. Code issu de Microsoft Windows-driver-samples
(commit `f4c1ff8`) : MS-PL, voir `MicroStudioCable/LICENSE-MS-PL.txt`.
