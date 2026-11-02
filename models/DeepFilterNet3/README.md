# Modèle DeepFilterNet3

Poids entraînés de **DeepFilterNet3** (Hendrik Schröter et al., « DeepFilterNet:
Perceptually Motivated Real-Time Speech Enhancement », Interspeech 2023),
copiés sans modification depuis le dépôt officiel
[Rikorose/DeepFilterNet](https://github.com/Rikorose/DeepFilterNet), fichier
`models/DeepFilterNet3_onnx.tar.gz`, commit
[`d375b2d`](https://github.com/Rikorose/DeepFilterNet/tree/d375b2d8309e0935d165700c91da9de862a99c31).

Licence : MIT ou Apache-2.0 au choix (`LICENSE-MIT`, `LICENSE-APACHE`).

Seuls les poids viennent de là : le moteur qui les exécute (`src/dsp/dfn/`),
lecteur ONNX compris, est écrit pour Micro Studio.

| Fichier        | SHA-256                                                            |
|----------------|--------------------------------------------------------------------|
| `enc.onnx`     | `7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916` |
| `erb_dec.onnx` | `ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895` |
| `df_dec.onnx`  | `23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a` |

Pour vérifier : `tar xzf DeepFilterNet3_onnx.tar.gz && sha256sum */*.onnx`.
