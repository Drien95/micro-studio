/*++

Module Name:

    cable.cpp

Abstract:

    Micro Studio Cable : enveloppe noyau de driver/core/ms_loopback.c.
    Voir cable.h.

    SPDX-License-Identifier: MIT
--*/
#include "definitions.h"
#include "cable.h"
#include "..\..\..\core\ms_loopback.h"

// Mémoire non paginée (section .data par défaut) : accessible à DISPATCH_LEVEL.
static UCHAR        g_CableStorage[MSCABLE_CAPACITY_BYTES];
static MsLoopback   g_Cable;
static KSPIN_LOCK   g_CableLock;
static BOOLEAN      g_CableReady = FALSE;

//=============================================================================
// Traduit un WAVEFORMATEX(TENSIBLE) PCM entier en MsPcmFormat.
// Renvoie FALSE pour tout autre format (flottant, compressé...).
#pragma code_seg()
static BOOLEAN MsCableFormatFromWave
(
    _In_  const WAVEFORMATEX* Wave,
    _Out_ MsPcmFormat*        Pcm
)
{
    Pcm->channels = Wave->nChannels;
    Pcm->bitsPerSample = Wave->wBitsPerSample;

    if (Wave->wFormatTag == WAVE_FORMAT_EXTENSIBLE)
    {
        const WAVEFORMATEXTENSIBLE* ext = (const WAVEFORMATEXTENSIBLE*)Wave;
        if (!IsEqualGUIDAligned(ext->SubFormat, KSDATAFORMAT_SUBTYPE_PCM))
        {
            return FALSE;
        }
    }
    else if (Wave->wFormatTag != WAVE_FORMAT_PCM)
    {
        return FALSE;
    }
    return MsPcmFormatIsValid(Pcm) ? TRUE : FALSE;
}

//=============================================================================
#pragma code_seg("PAGE")
NTSTATUS MsCableInitialize()
{
    PAGED_CODE();

    KeInitializeSpinLock(&g_CableLock);
    MsStatus status = MsLoopbackInit(&g_Cable, g_CableStorage,
                                     sizeof(g_CableStorage), MSCABLE_PRIME_BYTES);
    if (status != MS_OK)
    {
        return STATUS_INVALID_PARAMETER;
    }
    g_CableReady = TRUE;
    return STATUS_SUCCESS;
}

//=============================================================================
#pragma code_seg()
VOID MsCableWrite
(
    _In_reads_bytes_(Bytes) const BYTE* Data,
    _In_ ULONG Bytes,
    _In_ const WAVEFORMATEX* Format
)
{
    MsPcmFormat pcm;
    KIRQL oldIrql;

    if (!g_CableReady || Data == NULL || Format == NULL ||
        !MsCableFormatFromWave(Format, &pcm))
    {
        return;
    }
    KeAcquireSpinLock(&g_CableLock, &oldIrql);
    (void)MsLoopbackWritePcm(&g_Cable, Data, Bytes, &pcm);
    KeReleaseSpinLock(&g_CableLock, oldIrql);
}

//=============================================================================
#pragma code_seg()
VOID MsCableRead
(
    _Out_writes_bytes_(Bytes) BYTE* Data,
    _In_ ULONG Bytes,
    _In_ const WAVEFORMATEX* Format
)
{
    MsPcmFormat pcm;
    KIRQL oldIrql;

    if (Data == NULL)
    {
        return;
    }
    if (!g_CableReady || Format == NULL || !MsCableFormatFromWave(Format, &pcm))
    {
        RtlZeroMemory(Data, Bytes);
        return;
    }
    KeAcquireSpinLock(&g_CableLock, &oldIrql);
    (void)MsLoopbackReadPcm(&g_Cable, Data, Bytes, &pcm, NULL);
    KeReleaseSpinLock(&g_CableLock, oldIrql);
}

//=============================================================================
#pragma code_seg()
VOID MsCableCaptureStarted()
{
    KIRQL oldIrql;

    if (!g_CableReady)
    {
        return;
    }
    KeAcquireSpinLock(&g_CableLock, &oldIrql);
    (void)MsLoopbackResync(&g_Cable, MSCABLE_PRIME_BYTES);
    KeReleaseSpinLock(&g_CableLock, oldIrql);
}
