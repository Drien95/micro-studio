/*
 * ms_loopback.c - Implémentation de la boucle audio (voir ms_loopback.h).
 *
 * SPDX-License-Identifier: MIT
 */
#include "ms_loopback.h"

#include <string.h>

/* Échantillon canonique : entier signé 16 bits stocké dans un int. */
typedef struct MsFrame {
    int left;
    int right;
} MsFrame;

/* ------------------------------------------------------------------------- */
/* Conversions d'échantillons (petit-boutiste, sans dépendre de l'hôte).     */
/* ------------------------------------------------------------------------- */

static int MsDecodeS16(unsigned char lo, unsigned char hi)
{
    int value = (int)((unsigned int)lo | ((unsigned int)hi << 8));
    if (value >= 32768) {
        value -= 65536;
    }
    return value;
}

static void MsEncodeS16(int value, unsigned char *out)
{
    unsigned int bits = (unsigned int)(value < 0 ? value + 65536 : value);
    out[0] = (unsigned char)(bits & 0xFFu);
    out[1] = (unsigned char)((bits >> 8) & 0xFFu);
}

/* Lit un échantillon et le ramène à 16 bits (on garde les bits de poids fort). */
static int MsReadSample(const unsigned char *p, unsigned int bits)
{
    size_t bytes = (size_t)(bits / 8u);
    return MsDecodeS16(p[bytes - 2u], p[bytes - 1u]);
}

/* Écrit un échantillon 16 bits dans un conteneur de bits/8 octets. */
static void MsWriteSample(int value, unsigned char *p, unsigned int bits)
{
    size_t bytes = (size_t)(bits / 8u);
    size_t i;
    for (i = 0u; i + 2u < bytes; ++i) {
        p[i] = 0u;
    }
    MsEncodeS16(value, p + bytes - 2u);
}

static MsFrame MsDecodeFrame(const unsigned char *p, const MsPcmFormat *fmt)
{
    MsFrame f;
    size_t sampleBytes = (size_t)(fmt->bitsPerSample / 8u);
    f.left = MsReadSample(p, fmt->bitsPerSample);
    f.right = (fmt->channels == 2u)
                  ? MsReadSample(p + sampleBytes, fmt->bitsPerSample)
                  : f.left;
    return f;
}

static void MsEncodeFrame(MsFrame f, unsigned char *p, const MsPcmFormat *fmt)
{
    size_t sampleBytes = (size_t)(fmt->bitsPerSample / 8u);
    if (fmt->channels == 2u) {
        MsWriteSample(f.left, p, fmt->bitsPerSample);
        MsWriteSample(f.right, p + sampleBytes, fmt->bitsPerSample);
    } else {
        /* Mixage mono : moyenne, arrondie vers zéro, toujours dans [-32768, 32767]. */
        MsWriteSample((f.left + f.right) / 2, p, fmt->bitsPerSample);
    }
}

/* ------------------------------------------------------------------------- */
/* Accès au tampon circulaire (une trame canonique à la fois).               */
/* ------------------------------------------------------------------------- */

static void MsDropOldestFrame(MsLoopback *lb)
{
    lb->readPos = (lb->readPos + MS_CANON_FRAME_BYTES) % lb->capacity;
    lb->fill -= MS_CANON_FRAME_BYTES;
    lb->stats.droppedBytes += MS_CANON_FRAME_BYTES;
}

static void MsPushFrame(MsLoopback *lb, MsFrame f)
{
    size_t writePos;
    if (lb->fill == lb->capacity) {
        MsDropOldestFrame(lb);
    }
    writePos = (lb->readPos + lb->fill) % lb->capacity;
    MsEncodeS16(f.left, lb->storage + writePos);
    MsEncodeS16(f.right, lb->storage + writePos + 2u);
    lb->fill += MS_CANON_FRAME_BYTES;
}

static MsFrame MsPopFrame(MsLoopback *lb)
{
    MsFrame f;
    const unsigned char *p = lb->storage + lb->readPos;
    f.left = MsDecodeS16(p[0], p[1]);
    f.right = MsDecodeS16(p[2], p[3]);
    lb->readPos = (lb->readPos + MS_CANON_FRAME_BYTES) % lb->capacity;
    lb->fill -= MS_CANON_FRAME_BYTES;
    return f;
}

/* ------------------------------------------------------------------------- */
/* API publique.                                                             */
/* ------------------------------------------------------------------------- */

int MsPcmFormatIsValid(const MsPcmFormat *fmt)
{
    if (fmt == NULL) {
        return 0;
    }
    if (fmt->channels != 1u && fmt->channels != 2u) {
        return 0;
    }
    return (fmt->bitsPerSample == 16u || fmt->bitsPerSample == 24u ||
            fmt->bitsPerSample == 32u)
               ? 1
               : 0;
}

size_t MsPcmFrameBytes(const MsPcmFormat *fmt)
{
    if (!MsPcmFormatIsValid(fmt)) {
        return 0u;
    }
    return (size_t)fmt->channels * (size_t)(fmt->bitsPerSample / 8u);
}

MsStatus MsLoopbackInit(MsLoopback *lb, unsigned char *storage,
                        size_t capacity, size_t primeBytes)
{
    if (lb == NULL || storage == NULL) {
        return MS_ERR_NULL;
    }
    if (capacity == 0u || (capacity % MS_CANON_FRAME_BYTES) != 0u ||
        (primeBytes % MS_CANON_FRAME_BYTES) != 0u || primeBytes >= capacity) {
        return MS_ERR_SIZE;
    }
    lb->storage = storage;
    lb->capacity = capacity;
    lb->primeBytes = primeBytes;
    lb->stats.droppedBytes = 0u;
    lb->stats.underruns = 0u;
    memset(storage, 0, capacity);
    return MsLoopbackReset(lb);
}

MsStatus MsLoopbackReset(MsLoopback *lb)
{
    if (lb == NULL || lb->storage == NULL) {
        return MS_ERR_NULL;
    }
    lb->readPos = 0u;
    lb->fill = 0u;
    lb->primed = 0;
    return MS_OK;
}

MsStatus MsLoopbackResync(MsLoopback *lb, size_t keepBytes)
{
    size_t keep;
    if (lb == NULL || lb->storage == NULL) {
        return MS_ERR_NULL;
    }
    keep = keepBytes - (keepBytes % MS_CANON_FRAME_BYTES);
    if (lb->fill > keep) {
        size_t drop = lb->fill - keep;
        lb->readPos = (lb->readPos + drop) % lb->capacity;
        lb->fill = keep;
    }
    return MS_OK;
}

MsStatus MsLoopbackWritePcm(MsLoopback *lb, const unsigned char *src,
                            size_t srcBytes, const MsPcmFormat *fmt)
{
    size_t frameBytes;
    size_t frames;
    size_t i;
    if (lb == NULL || lb->storage == NULL || src == NULL || fmt == NULL) {
        return MS_ERR_NULL;
    }
    frameBytes = MsPcmFrameBytes(fmt);
    if (frameBytes == 0u) {
        return MS_ERR_FORMAT;
    }
    frames = srcBytes / frameBytes;
    for (i = 0u; i < frames; ++i) {
        MsPushFrame(lb, MsDecodeFrame(src + (i * frameBytes), fmt));
    }
    return MS_OK;
}

MsStatus MsLoopbackReadPcm(MsLoopback *lb, unsigned char *dst,
                           size_t dstBytes, const MsPcmFormat *fmt,
                           size_t *framesRead)
{
    size_t frameBytes;
    size_t frames;
    size_t taken = 0u;
    size_t i;
    if (lb == NULL || lb->storage == NULL || dst == NULL || fmt == NULL) {
        return MS_ERR_NULL;
    }
    frameBytes = MsPcmFrameBytes(fmt);
    if (frameBytes == 0u) {
        return MS_ERR_FORMAT;
    }
    /* Silence par défaut : couvre l'amorçage, les sous-débits et une
     * éventuelle trame incomplète en fin de tampon. */
    memset(dst, 0, dstBytes);
    frames = dstBytes / frameBytes;

    if (!lb->primed && lb->fill >= lb->primeBytes && lb->fill > 0u) {
        lb->primed = 1;
    }
    if (lb->primed) {
        for (i = 0u; i < frames && lb->fill > 0u; ++i) {
            MsEncodeFrame(MsPopFrame(lb), dst + (i * frameBytes), fmt);
            ++taken;
        }
        if (taken < frames) {
            /* Sous-débit : on se réamorce pour éviter une suite de clics. */
            lb->primed = 0;
            lb->stats.underruns += 1u;
        }
    }
    if (framesRead != NULL) {
        *framesRead = taken;
    }
    return MS_OK;
}
