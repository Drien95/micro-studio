# Contribuer à Micro Studio

## Avant chaque commit

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

La CI refuse toute pull request où l'une de ces commandes échoue.

## Règles de code

- **Temps réel** : dans les callbacks audio (`engine.rs`) et tout ce qu'ils
  appellent (`Chain::process`, `Chain::set_params`, `Resampler`), pas
  d'allocation mémoire, pas de verrou bloquant (`try_lock` uniquement), pas
  d'entrée/sortie, pas de `println!`. `tests/realtime.rs` vérifie les
  allocations.
- **Pas de plantage volontaire** hors tests : pas de `unwrap()`, de `panic!`
  ni d'indexation non vérifiée sur des données venant de l'extérieur (pilote
  audio, fichier de réglages). `expect("raison")` seulement pour un invariant
  prouvé localement.
- **Pas de `unsafe`** (interdit par le lint `unsafe_code`).
- Toute exception à un lint se fait avec `#[expect(lint, reason = "...")]`.
- Chaque correction de bug arrive avec un test qui l'aurait détecté.

## Messages de commit

Format [Conventional Commits](https://www.conventionalcommits.org/fr/v1.0.0/) :

```
<type>(<portée facultative>): <résumé à l'impératif>
```

Types : `feat` (fonctionnalité), `fix` (correction), `perf`, `refactor`,
`test`, `docs`, `ci`, `build`, `chore`. Un changement incompatible ajoute `!`
après le type (`feat!: ...`) et une ligne `BREAKING CHANGE:` dans le corps.

## Branches et versions

- `main` est toujours compilable et testée ; on y fusionne par pull request.
- Versionnage [SemVer](https://semver.org/lang/fr/) : `fix` → correctif
  (0.1.**1**), `feat` → mineure (0.**2**.0), changement incompatible → majeure.
  Avant la 1.0, une incompatibilité (ex. format de réglages) monte la mineure.
- Pour publier : mettre à jour `version` dans `Cargo.toml`, déplacer la section
  « Non publié » de `CHANGELOG.md` sous le nouveau numéro, fusionner, puis
  pousser le tag `vX.Y.Z`. La CI vérifie que le tag et `Cargo.toml` concordent,
  compile l'exécutable et crée la release avec les notes du changelog.
- Correctif urgent sur une version publiée : branche `hotfix/X.Y.Z` depuis le
  tag, correction + test, tag `vX.Y.(Z+1)`, puis report sur `main`.
