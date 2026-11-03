# Modèle DNSMOS P.835

Poids entraînés de **DNSMOS P.835** (Chandan K. A. Reddy, Vishak Gopal, Ross
Cutler, « DNSMOS P.835: A non-intrusive perceptual objective speech quality
metric to evaluate noise suppressors », ICASSP 2022), copiés sans
modification depuis le dépôt officiel de Microsoft
[microsoft/DNS-Challenge](https://github.com/microsoft/DNS-Challenge), fichier
`DNSMOS/DNSMOS/sig_bak_ovr.onnx`, commit
[`591184a`](https://github.com/microsoft/DNS-Challenge/tree/591184a9fcb2cbdec02520fed81a32bbbf9d73ff).

Licence : Creative Commons Attribution 4.0 International (`LICENSE-CC-BY-4.0`),
celle du dépôt pour tout ce qui n'est pas du code. © Microsoft Corporation.

Le réseau prédit, sans enregistrement de référence, les trois notes de la
recommandation ITU-T P.835 que donneraient des auditeurs : qualité de la voix
(SIG), discrétion du bruit de fond (BAK) et note globale (OVRL), de 1 à 5.

Seuls les poids viennent de là : le code qui les exécute (`src/dnsmos.rs`),
lecteur ONNX compris, est écrit pour Micro Studio.

| Fichier            | SHA-256                                                            |
|--------------------|--------------------------------------------------------------------|
| `sig_bak_ovr.onnx` | `269fbebdb513aa23cddfbb593542ecc540284a91849ac50516870e1ac78f6edd` |
