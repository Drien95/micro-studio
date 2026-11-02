## Quoi et pourquoi

<!-- Le problème résolu, en une ou deux phrases. -->

## Vérifications

- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` et `cargo test` passent
- [ ] Rien n'alloue ni ne bloque dans les callbacks audio (`tests/realtime.rs`)
- [ ] Tests ajoutés ou mis à jour pour le comportement modifié
- [ ] `CHANGELOG.md` mis à jour (section « Non publié »)
- [ ] Testé avec un vrai micro, si le son ou le moteur audio change
