#pragma once
#include "rta_fft.h"

#define RTA_BASS_BANDS 14
#define RTA_BASS_SOS 4
#define RTA_BASS_BAND_SOS 3 /* 6th-order Butterworth bandpass per band */
#if RTA_SAMPLE_FLOAT
typedef float rta_bass_value_t;
#else
typedef int32_t rta_bass_value_t; /* state Q27, coefficients Q28 */
#endif

typedef struct {
    rta_bass_value_t b0, b1, a1, a2; /* numerator b0 + b1*z^-1 + b0*z^-2 */
} RtaBassSos;
typedef struct {
    rta_bass_value_t b0, a1, a2; /* numerator b0*(1 - z^-2) */
} RtaBassBandSos;
typedef struct {
    RtaBassBandSos sos[RTA_BASS_BAND_SOS];
    rta_bass_value_t envelope;
    float compensation;
} RtaBassBandCoeffs;
typedef struct {
    uint8_t cic_shift, decimation;
    RtaBassSos lowpass[RTA_BASS_SOS];
    RtaBassBandCoeffs band[RTA_BASS_BANDS];
} RtaBassCoeffs;
typedef struct {
    uint32_t integrator[3], comb[3]; /* unsigned modular CIC arithmetic */
    rta_bass_value_t lp[RTA_BASS_SOS][2];
    struct {
        rta_bass_value_t z[RTA_BASS_BAND_SOS][2];
#if RTA_SAMPLE_FLOAT
        float power;
#else
        int64_t power; /* Q48 squared amplitude; wide to preserve quiet bass */
#endif
    } band[RTA_BASS_BANDS];
    uint8_t count, low_count;
} RtaBassChannel;

/* Main-loop configuration/readout; c is RAM, shared read-only by both cores. */
void rta_bass_configure(RtaBassCoeffs *c, uint32_t rate, uint16_t avg_ms);
float rta_bass_power(const RtaBassCoeffs *c, const RtaBassChannel *s, uint8_t band);
/* One owning core per channel, every sample, even during FFT work. */
void rta_bass_push(const RtaBassCoeffs *c, RtaBassChannel *s,
#if RTA_SAMPLE_FLOAT
                   const float *samples,
#else
                   const int32_t *samples, /* audio Q28 */
#endif
                   uint32_t n);
