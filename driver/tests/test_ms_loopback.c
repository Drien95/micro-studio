/*
 * test_ms_loopback.c - Tests unitaires et de régression du cœur de la boucle.
 *
 * Organisation (ISO/IEC/IEEE 29119-4) :
 *   - partitions d'équivalence et valeurs limites sur les arguments ;
 *   - transitions d'état (non amorcé -> amorcé -> sous-débit -> non amorcé) ;
 *   - test différentiel : 20 000 opérations pseudo-aléatoires comparées à un
 *     modèle de référence volontairement naïf (régression).
 *
 * Aucune dépendance : un exécutable qui renvoie 0 si tout passe.
 * SPDX-License-Identifier: MIT
 */
#include "../core/ms_loopback.h"

#include <stdio.h>
#include <string.h>

static int g_failures = 0;
static int g_checks = 0;

#define CHECK(cond)                                                          \
    do {                                                                     \
        ++g_checks;                                                          \
        if (!(cond)) {                                                       \
            ++g_failures;                                                    \
            printf("ÉCHEC %s:%d : %s\n", __FILE__, __LINE__, #cond);         \
        }                                                                    \
    } while (0)

static const MsPcmFormat S16_STEREO = {2u, 16u};
static const MsPcmFormat S16_MONO = {1u, 16u};
static const MsPcmFormat S24_STEREO = {2u, 24u};
static const MsPcmFormat S32_STEREO = {2u, 32u};
static const MsPcmFormat S32_MONO = {1u, 32u};

/* Construit une trame canonique 16 bits stéréo. */
static void MakeS16(unsigned char *p, int left, int right)
{
    unsigned int l = (unsigned int)(left < 0 ? left + 65536 : left);
    unsigned int r = (unsigned int)(right < 0 ? right + 65536 : right);
    p[0] = (unsigned char)(l & 0xFFu);
    p[1] = (unsigned char)(l >> 8);
    p[2] = (unsigned char)(r & 0xFFu);
    p[3] = (unsigned char)(r >> 8);
}

static int ReadS16(const unsigned char *p)
{
    int v = (int)((unsigned int)p[0] | ((unsigned int)p[1] << 8));
    return v >= 32768 ? v - 65536 : v;
}

static int AllZero(const unsigned char *p, size_t n)
{
    size_t i;
    for (i = 0u; i < n; ++i) {
        if (p[i] != 0u) {
            return 0;
        }
    }
    return 1;
}

/* ------------------------------------------------------------------------- */

static void TestInitArguments(void)
{
    MsLoopback lb;
    unsigned char mem[64];

    CHECK(MsLoopbackInit(NULL, mem, 64u, 0u) == MS_ERR_NULL);
    CHECK(MsLoopbackInit(&lb, NULL, 64u, 0u) == MS_ERR_NULL);
    CHECK(MsLoopbackInit(&lb, mem, 0u, 0u) == MS_ERR_SIZE);
    CHECK(MsLoopbackInit(&lb, mem, 63u, 0u) == MS_ERR_SIZE);
    CHECK(MsLoopbackInit(&lb, mem, 64u, 2u) == MS_ERR_SIZE);
    CHECK(MsLoopbackInit(&lb, mem, 64u, 64u) == MS_ERR_SIZE);
    CHECK(MsLoopbackInit(&lb, mem, 64u, 60u) == MS_OK);
    CHECK(MsLoopbackInit(&lb, mem, 4u, 0u) == MS_OK);
    CHECK(lb.fill == 0u && lb.primed == 0);
}

static void TestFormats(void)
{
    MsPcmFormat bad1 = {0u, 16u};
    MsPcmFormat bad2 = {3u, 16u};
    MsPcmFormat bad3 = {2u, 8u};
    MsPcmFormat bad4 = {2u, 20u};

    CHECK(MsPcmFormatIsValid(NULL) == 0);
    CHECK(MsPcmFormatIsValid(&bad1) == 0);
    CHECK(MsPcmFormatIsValid(&bad2) == 0);
    CHECK(MsPcmFormatIsValid(&bad3) == 0);
    CHECK(MsPcmFormatIsValid(&bad4) == 0);
    CHECK(MsPcmFrameBytes(&bad4) == 0u);
    CHECK(MsPcmFrameBytes(&S16_MONO) == 2u);
    CHECK(MsPcmFrameBytes(&S16_STEREO) == 4u);
    CHECK(MsPcmFrameBytes(&S24_STEREO) == 6u);
    CHECK(MsPcmFrameBytes(&S32_STEREO) == 8u);
    CHECK(MsPcmFrameBytes(&S32_MONO) == 4u);
}

static void TestNullAndFormatErrors(void)
{
    MsLoopback lb;
    MsLoopback empty;
    unsigned char mem[16];
    unsigned char buf[8] = {0};
    MsPcmFormat bad = {2u, 8u};

    memset(&empty, 0, sizeof(empty));
    CHECK(MsLoopbackInit(&lb, mem, 16u, 0u) == MS_OK);
    CHECK(MsLoopbackWritePcm(NULL, buf, 4u, &S16_STEREO) == MS_ERR_NULL);
    CHECK(MsLoopbackWritePcm(&empty, buf, 4u, &S16_STEREO) == MS_ERR_NULL);
    CHECK(MsLoopbackWritePcm(&lb, NULL, 4u, &S16_STEREO) == MS_ERR_NULL);
    CHECK(MsLoopbackWritePcm(&lb, buf, 4u, NULL) == MS_ERR_NULL);
    CHECK(MsLoopbackWritePcm(&lb, buf, 4u, &bad) == MS_ERR_FORMAT);
    CHECK(MsLoopbackReadPcm(NULL, buf, 4u, &S16_STEREO, NULL) == MS_ERR_NULL);
    CHECK(MsLoopbackReadPcm(&empty, buf, 4u, &S16_STEREO, NULL) == MS_ERR_NULL);
    CHECK(MsLoopbackReadPcm(&lb, NULL, 4u, &S16_STEREO, NULL) == MS_ERR_NULL);
    CHECK(MsLoopbackReadPcm(&lb, buf, 4u, NULL, NULL) == MS_ERR_NULL);
    CHECK(MsLoopbackReadPcm(&lb, buf, 4u, &bad, NULL) == MS_ERR_FORMAT);
    CHECK(MsLoopbackReset(NULL) == MS_ERR_NULL);
    CHECK(MsLoopbackReset(&empty) == MS_ERR_NULL);
    CHECK(MsLoopbackResync(NULL, 0u) == MS_ERR_NULL);
    CHECK(MsLoopbackResync(&empty, 0u) == MS_ERR_NULL);
}

static void TestRoundTripS16(void)
{
    MsLoopback lb;
    unsigned char mem[64];
    unsigned char in[12];
    unsigned char out[12];
    size_t got = 99u;

    CHECK(MsLoopbackInit(&lb, mem, 64u, 0u) == MS_OK);
    MakeS16(in, 1, -1);
    MakeS16(in + 4, 32767, -32768);
    MakeS16(in + 8, 1234, -4321);
    CHECK(MsLoopbackWritePcm(&lb, in, sizeof(in), &S16_STEREO) == MS_OK);
    CHECK(lb.fill == 12u);
    CHECK(MsLoopbackReadPcm(&lb, out, sizeof(out), &S16_STEREO, &got) == MS_OK);
    CHECK(got == 3u);
    CHECK(memcmp(in, out, sizeof(in)) == 0);
    CHECK(lb.fill == 0u);
}

static void TestConversionsToCapture(void)
{
    MsLoopback lb;
    unsigned char mem[64];
    unsigned char in[8];
    unsigned char out32[16];
    unsigned char out24[6];
    unsigned char mono16[2];
    unsigned char mono32[4];

    CHECK(MsLoopbackInit(&lb, mem, 64u, 0u) == MS_OK);
    MakeS16(in, 0x1234, -2);
    MakeS16(in + 4, -32768, 32767);
    CHECK(MsLoopbackWritePcm(&lb, in, 8u, &S16_STEREO) == MS_OK);

    /* 16 -> 32 bits : l'échantillon occupe les 16 bits de poids fort. */
    CHECK(MsLoopbackReadPcm(&lb, out32, 8u, &S32_STEREO, NULL) == MS_OK);
    CHECK(out32[0] == 0u && out32[1] == 0u);
    CHECK(out32[2] == 0x34u && out32[3] == 0x12u);
    CHECK(out32[4] == 0u && out32[5] == 0u);
    CHECK(out32[6] == 0xFEu && out32[7] == 0xFFu);

    /* 16 -> 24 bits. */
    CHECK(MsLoopbackReadPcm(&lb, out24, 6u, &S24_STEREO, NULL) == MS_OK);
    CHECK(out24[0] == 0u && out24[1] == 0x00u && out24[2] == 0x80u);
    CHECK(out24[3] == 0u && out24[4] == 0xFFu && out24[5] == 0x7Fu);

    /* Stéréo -> mono : moyenne, sans débordement aux extrêmes. */
    MakeS16(in, 1000, 3000);
    MakeS16(in + 4, -32768, -32768);
    CHECK(MsLoopbackWritePcm(&lb, in, 8u, &S16_STEREO) == MS_OK);
    CHECK(MsLoopbackReadPcm(&lb, mono16, 2u, &S16_MONO, NULL) == MS_OK);
    CHECK(ReadS16(mono16) == 2000);
    CHECK(MsLoopbackReadPcm(&lb, mono32, 4u, &S32_MONO, NULL) == MS_OK);
    CHECK(ReadS16(mono32 + 2) == -32768 && mono32[0] == 0u && mono32[1] == 0u);
}

static void TestConversionsFromRender(void)
{
    MsLoopback lb;
    unsigned char mem[64];
    unsigned char in32[8] = {0xAAu, 0xBBu, 0x34u, 0x12u, 0x00u, 0x00u, 0x00u, 0x80u};
    unsigned char in24[3] = {0x99u, 0xFFu, 0x7Fu};
    unsigned char in16mono[2];
    unsigned char out[4];
    MsPcmFormat s24mono = {1u, 24u};

    CHECK(MsLoopbackInit(&lb, mem, 64u, 0u) == MS_OK);
    /* 32 bits : les octets de poids faible sont tronqués. */
    CHECK(MsLoopbackWritePcm(&lb, in32, 8u, &S32_STEREO) == MS_OK);
    CHECK(MsLoopbackReadPcm(&lb, out, 4u, &S16_STEREO, NULL) == MS_OK);
    CHECK(ReadS16(out) == 0x1234 && ReadS16(out + 2) == -32768);
    /* Mono -> stéréo : duplication. */
    CHECK(MsLoopbackWritePcm(&lb, in24, 3u, &s24mono) == MS_OK);
    CHECK(MsLoopbackReadPcm(&lb, out, 4u, &S16_STEREO, NULL) == MS_OK);
    CHECK(ReadS16(out) == 32767 && ReadS16(out + 2) == 32767);
    in16mono[0] = 0x00u;
    in16mono[1] = 0xC0u;
    CHECK(MsLoopbackWritePcm(&lb, in16mono, 2u, &S16_MONO) == MS_OK);
    CHECK(MsLoopbackReadPcm(&lb, out, 4u, &S16_STEREO, NULL) == MS_OK);
    CHECK(ReadS16(out) == -16384 && ReadS16(out + 2) == -16384);
}

static void TestPartialFrames(void)
{
    MsLoopback lb;
    unsigned char mem[64];
    unsigned char in[7];
    unsigned char out[7];
    size_t got = 0u;

    CHECK(MsLoopbackInit(&lb, mem, 64u, 0u) == MS_OK);
    memset(in, 0x11, sizeof(in));
    CHECK(MsLoopbackWritePcm(&lb, in, 7u, &S16_STEREO) == MS_OK);
    CHECK(lb.fill == 4u); /* 3 octets de trame incomplète ignorés */
    memset(out, 0x55, sizeof(out));
    CHECK(MsLoopbackReadPcm(&lb, out, 7u, &S16_STEREO, &got) == MS_OK);
    CHECK(got == 1u);
    CHECK(AllZero(out + 4, 3u)); /* fin de tampon remplie de silence */
    CHECK(MsLoopbackWritePcm(&lb, in, 0u, &S16_STEREO) == MS_OK);
    CHECK(lb.fill == 0u);
}

static void TestWrapAround(void)
{
    MsLoopback lb;
    unsigned char mem[16]; /* 4 trames */
    unsigned char in[12];
    unsigned char out[12];
    int round;

    CHECK(MsLoopbackInit(&lb, mem, 16u, 0u) == MS_OK);
    for (round = 0; round < 10; ++round) {
        MakeS16(in, round, -round);
        MakeS16(in + 4, round + 100, -round - 100);
        MakeS16(in + 8, round + 200, -round - 200);
        CHECK(MsLoopbackWritePcm(&lb, in, 12u, &S16_STEREO) == MS_OK);
        CHECK(MsLoopbackReadPcm(&lb, out, 12u, &S16_STEREO, NULL) == MS_OK);
        CHECK(memcmp(in, out, 12u) == 0);
    }
    CHECK(lb.stats.droppedBytes == 0u && lb.stats.underruns == 0u);
}

static void TestOverflowDropsOldest(void)
{
    MsLoopback lb;
    unsigned char mem[8]; /* 2 trames */
    unsigned char in[12];
    unsigned char out[8];

    CHECK(MsLoopbackInit(&lb, mem, 8u, 0u) == MS_OK);
    MakeS16(in, 1, 1);
    MakeS16(in + 4, 2, 2);
    MakeS16(in + 8, 3, 3);
    CHECK(MsLoopbackWritePcm(&lb, in, 12u, &S16_STEREO) == MS_OK);
    CHECK(lb.fill == 8u);
    CHECK(lb.stats.droppedBytes == 4u);
    CHECK(MsLoopbackReadPcm(&lb, out, 8u, &S16_STEREO, NULL) == MS_OK);
    CHECK(ReadS16(out) == 2 && ReadS16(out + 4) == 3);
}

static void TestPrimingAndUnderrun(void)
{
    MsLoopback lb;
    unsigned char mem[32];
    unsigned char in[8];
    unsigned char out[12];
    size_t got = 99u;

    /* Il faut 2 trames avant de rendre du son. */
    CHECK(MsLoopbackInit(&lb, mem, 32u, 8u) == MS_OK);
    MakeS16(in, 7, 7);
    MakeS16(in + 4, 8, 8);

    /* Vide : silence, pas de sous-débit compté (pas encore amorcé). */
    memset(out, 0x55, sizeof(out));
    CHECK(MsLoopbackReadPcm(&lb, out, 4u, &S16_STEREO, &got) == MS_OK);
    CHECK(got == 0u && AllZero(out, 4u) && lb.stats.underruns == 0u);

    /* Une trame : toujours sous le seuil, rien n'est consommé. */
    CHECK(MsLoopbackWritePcm(&lb, in, 4u, &S16_STEREO) == MS_OK);
    CHECK(MsLoopbackReadPcm(&lb, out, 4u, &S16_STEREO, &got) == MS_OK);
    CHECK(got == 0u && lb.fill == 4u && lb.primed == 0);

    /* Deux trames : amorcé, on lit 3 trames -> 2 réelles + silence. */
    CHECK(MsLoopbackWritePcm(&lb, in + 4, 4u, &S16_STEREO) == MS_OK);
    memset(out, 0x55, sizeof(out));
    CHECK(MsLoopbackReadPcm(&lb, out, 12u, &S16_STEREO, &got) == MS_OK);
    CHECK(got == 2u);
    CHECK(ReadS16(out) == 7 && ReadS16(out + 4) == 8 && AllZero(out + 8, 4u));
    CHECK(lb.stats.underruns == 1u && lb.primed == 0);

    /* Lecture de 0 octet : neutre. */
    CHECK(MsLoopbackReadPcm(&lb, out, 0u, &S16_STEREO, NULL) == MS_OK);
    CHECK(lb.stats.underruns == 1u);
}

static void TestResyncAndReset(void)
{
    MsLoopback lb;
    unsigned char mem[32];
    unsigned char in[16];
    unsigned char out[4];

    CHECK(MsLoopbackInit(&lb, mem, 32u, 0u) == MS_OK);
    MakeS16(in, 1, 1);
    MakeS16(in + 4, 2, 2);
    MakeS16(in + 8, 3, 3);
    MakeS16(in + 12, 4, 4);
    CHECK(MsLoopbackWritePcm(&lb, in, 16u, &S16_STEREO) == MS_OK);

    /* Garder 6 octets -> arrondi à 1 trame : la plus récente. */
    CHECK(MsLoopbackResync(&lb, 6u) == MS_OK);
    CHECK(lb.fill == 4u);
    CHECK(MsLoopbackReadPcm(&lb, out, 4u, &S16_STEREO, NULL) == MS_OK);
    CHECK(ReadS16(out) == 4);

    /* Resync plus grand que le contenu : sans effet. */
    CHECK(MsLoopbackWritePcm(&lb, in, 8u, &S16_STEREO) == MS_OK);
    CHECK(MsLoopbackResync(&lb, 100u) == MS_OK);
    CHECK(lb.fill == 8u);

    CHECK(MsLoopbackReset(&lb) == MS_OK);
    CHECK(lb.fill == 0u && lb.primed == 0);
}

/* ------------------------------------------------------------------------- */
/* Test différentiel contre un modèle de référence (régression).             */
/* ------------------------------------------------------------------------- */

#define MODEL_FRAMES 64u
#define HISTORY_FRAMES 100000u

typedef struct Model {
    int left[HISTORY_FRAMES];
    int right[HISTORY_FRAMES];
    size_t head;   /* première trame non lue */
    size_t tail;   /* prochaine trame écrite */
} Model;

static Model g_model;
static unsigned int g_seed = 12345u;

static unsigned int NextRand(void)
{
    g_seed = g_seed * 1103515245u + 12345u;
    return (g_seed >> 16) & 0x7FFFu;
}

static void TestDifferential(void)
{
    MsLoopback lb;
    unsigned char mem[MODEL_FRAMES * 4u];
    unsigned char buf[40u * 4u];
    int step;
    int mismatch = 0;

    memset(&g_model, 0, sizeof(g_model));
    CHECK(MsLoopbackInit(&lb, mem, sizeof(mem), 0u) == MS_OK);
    for (step = 0; step < 20000 && g_model.tail + 40u < HISTORY_FRAMES; ++step) {
        size_t n = (size_t)(NextRand() % 40u);
        size_t i;
        if ((NextRand() & 1u) == 0u) {
            for (i = 0u; i < n; ++i) {
                int l = (int)(NextRand() % 65536u) - 32768;
                int r = (int)(NextRand() % 65536u) - 32768;
                MakeS16(buf + (i * 4u), l, r);
                g_model.left[g_model.tail] = l;
                g_model.right[g_model.tail] = r;
                ++g_model.tail;
            }
            if (g_model.tail - g_model.head > MODEL_FRAMES) {
                g_model.head = g_model.tail - MODEL_FRAMES;
            }
            CHECK(MsLoopbackWritePcm(&lb, buf, n * 4u, &S16_STEREO) == MS_OK);
        } else {
            size_t got = 0u;
            CHECK(MsLoopbackReadPcm(&lb, buf, n * 4u, &S16_STEREO, &got) == MS_OK);
            for (i = 0u; i < n; ++i) {
                int el = 0;
                int er = 0;
                if (g_model.head < g_model.tail) {
                    el = g_model.left[g_model.head];
                    er = g_model.right[g_model.head];
                    ++g_model.head;
                }
                if (ReadS16(buf + (i * 4u)) != el || ReadS16(buf + (i * 4u) + 2u) != er) {
                    mismatch = 1;
                }
            }
            (void)got;
        }
        if (lb.fill != (g_model.tail - g_model.head) * 4u) {
            mismatch = 1;
        }
    }
    CHECK(mismatch == 0);
}

int main(void)
{
    TestInitArguments();
    TestFormats();
    TestNullAndFormatErrors();
    TestRoundTripS16();
    TestConversionsToCapture();
    TestConversionsFromRender();
    TestPartialFrames();
    TestWrapAround();
    TestOverflowDropsOldest();
    TestPrimingAndUnderrun();
    TestResyncAndReset();
    TestDifferential();

    printf("%d vérifications, %d échec(s)\n", g_checks, g_failures);
    return g_failures == 0 ? 0 : 1;
}
