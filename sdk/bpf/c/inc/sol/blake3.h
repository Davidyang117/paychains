#pragma once
/**
 * @brief PayChains Blake3 system call
 */

#include <sol/types.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Length of a Blake3 hash result
 */
#define BLAKE3_RESULT_LENGTH 32

/**
 * Blake3
 *
 * @param bytes Array of byte arrays
 * @param bytes_len Number of byte arrays
 * @param result 32 byte array to hold the result
 */
uint64_t pay_blake3(
    const PayBytes *bytes,
    int bytes_len,
    const uint8_t *result
);

#ifdef __cplusplus
}
#endif

/**@}*/
