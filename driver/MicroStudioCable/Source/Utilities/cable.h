/*++

Module Name:

    cable.h

Abstract:

    Micro Studio Cable : relie le haut-parleur virtuel au micro virtuel.

    Tout ce que Windows envoie au haut-parleur « Micro Studio » ressort,
    sans modification, du micro « Micro Studio ». La logique du tampon et
    des conversions est dans driver/core/ms_loopback.c (testée hors noyau) ;
    ce module n'ajoute que le stockage non paginé et le verrou.

    Toutes les fonctions sont appelables jusqu'à DISPATCH_LEVEL.

    SPDX-License-Identifier: MIT
--*/
#ifndef _MICROSTUDIO_CABLE_H_
#define _MICROSTUDIO_CABLE_H_

// 200 ms à 48 kHz, 16 bits stéréo : latence maximale avant écrasement.
#define MSCABLE_CAPACITY_BYTES   (48000 * 4 / 5)
// 10 ms de réserve avant de rendre du son (absorbe la gigue entre les flux).
#define MSCABLE_PRIME_BYTES      (48000 * 4 / 100)

NTSTATUS MsCableInitialize();

// Côté haut-parleur : copie les octets lus dans le tampon DMA de rendu.
VOID MsCableWrite(_In_reads_bytes_(Bytes) const BYTE* Data,
                  _In_ ULONG Bytes,
                  _In_ const WAVEFORMATEX* Format);

// Côté micro : remplit le tampon DMA de capture (silence si rien à lire).
VOID MsCableRead(_Out_writes_bytes_(Bytes) BYTE* Data,
                 _In_ ULONG Bytes,
                 _In_ const WAVEFORMATEX* Format);

// À appeler quand le micro passe en marche : jette l'audio périmé.
VOID MsCableCaptureStarted();

#endif // _MICROSTUDIO_CABLE_H_
