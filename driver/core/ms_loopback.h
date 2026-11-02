/*
 * ms_loopback.h - Boucle audio « haut-parleur virtuel -> micro virtuel ».
 *
 * Cœur portable du pilote Micro Studio Cable. Ce module ne dépend que de la
 * bibliothèque C standard minimale (stddef.h, string.h) : il est compilé tel
 * quel dans le pilote noyau Windows et dans les tests unitaires (Linux/Windows).
 *
 * Règles appliquées (NASA/JPL « Power of 10 ») :
 *   - aucune allocation dynamique : le stockage est fourni par l'appelant ;
 *   - aucune récursion, toutes les boucles sont bornées par une taille connue ;
 *   - chaque fonction publique vérifie ses arguments et renvoie un statut ;
 *   - aucune synchronisation interne : l'appelant protège l'objet (spinlock
 *     noyau dans le pilote), ce qui garde ce code déterministe et testable.
 *
 * Format interne (« canonique ») : PCM entier 16 bits signé, stéréo entrelacé,
 * petit-boutiste, 4 octets par trame. La fréquence d'échantillonnage n'est pas
 * convertie : les deux extrémités doivent utiliser la même (48 kHz).
 *
 * SPDX-License-Identifier: MIT
 */
#ifndef MS_LOOPBACK_H
#define MS_LOOPBACK_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Octets par trame du format canonique (2 canaux x 16 bits). */
#define MS_CANON_FRAME_BYTES ((size_t)4u)

/** Codes de retour. */
typedef enum MsStatus {
    MS_OK = 0,
    MS_ERR_NULL = -1,      /**< Pointeur obligatoire nul. */
    MS_ERR_SIZE = -2,      /**< Taille nulle ou non multiple d'une trame. */
    MS_ERR_FORMAT = -3     /**< Format PCM non pris en charge. */
} MsStatus;

/** Description d'un format PCM entier petit-boutiste. */
typedef struct MsPcmFormat {
    unsigned int channels;      /**< 1 (mono) ou 2 (stéréo). */
    unsigned int bitsPerSample; /**< 16, 24 ou 32 (conteneur = bits / 8). */
} MsPcmFormat;

/** Compteurs de diagnostic (cumulés depuis l'initialisation). */
typedef struct MsLoopbackStats {
    size_t droppedBytes;   /**< Octets canoniques perdus par débordement. */
    size_t underruns;      /**< Lectures servies partiellement (silence ajouté). */
} MsLoopbackStats;

/**
 * Tampon circulaire. Les champs sont publics pour permettre une allocation
 * statique, mais ne doivent être modifiés que via les fonctions ci-dessous.
 */
typedef struct MsLoopback {
    unsigned char *storage; /**< Mémoire fournie par l'appelant. */
    size_t capacity;        /**< Octets, multiple de MS_CANON_FRAME_BYTES. */
    size_t readPos;         /**< Index de lecture dans storage. */
    size_t fill;            /**< Octets disponibles à la lecture. */
    size_t primeBytes;      /**< Remplissage requis avant de (re)lire. */
    int primed;             /**< 1 si la lecture est amorcée. */
    MsLoopbackStats stats;
} MsLoopback;

/**
 * Initialise la boucle.
 * @param capacity   taille de storage en octets (multiple de 4, non nulle).
 * @param primeBytes remplissage à atteindre avant de rendre du son, pour
 *                   absorber la gigue entre les deux flux (multiple de 4,
 *                   strictement inférieur à capacity).
 */
MsStatus MsLoopbackInit(MsLoopback *lb, unsigned char *storage,
                        size_t capacity, size_t primeBytes);

/** Vide la boucle (désamorce la lecture). Les statistiques sont conservées. */
MsStatus MsLoopbackReset(MsLoopback *lb);

/**
 * Ne garde que les keepBytes les plus récents (arrondi à la trame). Utilisé
 * au démarrage du micro pour ne pas rejouer d'anciens échantillons.
 */
MsStatus MsLoopbackResync(MsLoopback *lb, size_t keepBytes);

/** Vrai si le format est pris en charge par les conversions. */
int MsPcmFormatIsValid(const MsPcmFormat *fmt);

/** Taille d'une trame du format (0 si format invalide). */
size_t MsPcmFrameBytes(const MsPcmFormat *fmt);

/**
 * Écrit des échantillons dans la boucle (côté haut-parleur virtuel).
 * Les octets d'une trame incomplète en fin de src sont ignorés. En cas de
 * débordement, les échantillons les PLUS ANCIENS sont écartés pour borner la
 * latence ; ils sont comptés dans stats.droppedBytes.
 */
MsStatus MsLoopbackWritePcm(MsLoopback *lb, const unsigned char *src,
                            size_t srcBytes, const MsPcmFormat *fmt);

/**
 * Lit des échantillons (côté micro virtuel) dans le format demandé.
 * dst est toujours entièrement rempli : ce qui manque est remplacé par du
 * silence (zéros). @param framesRead reçoit le nombre de trames réellement
 * issues de la boucle (peut être NULL).
 */
MsStatus MsLoopbackReadPcm(MsLoopback *lb, unsigned char *dst,
                           size_t dstBytes, const MsPcmFormat *fmt,
                           size_t *framesRead);

#ifdef __cplusplus
}
#endif

#endif /* MS_LOOPBACK_H */
