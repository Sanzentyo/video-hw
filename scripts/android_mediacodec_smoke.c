#include <media/NdkMediaCodec.h>
#include <media/NdkMediaFormat.h>

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

enum {
    WIDTH = 320,
    HEIGHT = 180,
    FPS = 30,
    FRAME_COUNT = 30,
    BITRATE = 1000000,
    TIMEOUT_US = 10000,
    COLOR_FORMAT_YUV420_SEMIPLANAR = 21,
};

typedef struct {
    uint8_t *data;
    size_t size;
    int64_t pts_us;
    uint32_t flags;
} Packet;

typedef struct {
    Packet *items;
    size_t len;
    size_t cap;
    size_t total_bytes;
    int keyframes;
    int codec_config_count;
} PacketList;

static void free_packets(PacketList *packets) {
    for (size_t i = 0; i < packets->len; i++) {
        free(packets->items[i].data);
    }
    free(packets->items);
    memset(packets, 0, sizeof(*packets));
}

static int push_packet(PacketList *packets, const uint8_t *data, size_t size, int64_t pts_us,
                       uint32_t flags) {
    if (size == 0) {
        return 0;
    }
    if (packets->len == packets->cap) {
        size_t next_cap = packets->cap == 0 ? 32 : packets->cap * 2;
        Packet *next = (Packet *)realloc(packets->items, next_cap * sizeof(Packet));
        if (!next) {
            return -1;
        }
        packets->items = next;
        packets->cap = next_cap;
    }
    uint8_t *copy = (uint8_t *)malloc(size);
    if (!copy) {
        return -1;
    }
    memcpy(copy, data, size);
    packets->items[packets->len++] = (Packet){
        .data = copy,
        .size = size,
        .pts_us = pts_us,
        .flags = flags,
    };
    packets->total_bytes += size;
    if ((flags & AMEDIACODEC_BUFFER_FLAG_KEY_FRAME) != 0) {
        packets->keyframes += 1;
    }
    if ((flags & AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG) != 0) {
        packets->codec_config_count += 1;
    }
    return 0;
}

static int write_packets(const char *path, const PacketList *packets) {
    FILE *file = fopen(path, "wb");
    if (!file) {
        fprintf(stderr, "failed to open %s: %s\n", path, strerror(errno));
        return -1;
    }
    for (size_t i = 0; i < packets->len; i++) {
        if (fwrite(packets->items[i].data, 1, packets->items[i].size, file) != packets->items[i].size) {
            fprintf(stderr, "failed to write %s\n", path);
            fclose(file);
            return -1;
        }
    }
    fclose(file);
    return 0;
}

static void fill_nv12(uint8_t *dst, size_t size, int frame_index) {
    const size_t y_size = WIDTH * HEIGHT;
    const size_t uv_size = y_size / 2;
    if (size < y_size + uv_size) {
        return;
    }

    for (int y = 0; y < HEIGHT; y++) {
        for (int x = 0; x < WIDTH; x++) {
            dst[y * WIDTH + x] = (uint8_t)((x + y + frame_index * 7) & 0xff);
        }
    }
    for (size_t i = 0; i < uv_size; i += 2) {
        dst[y_size + i] = (uint8_t)(96 + ((frame_index * 3) & 31));
        dst[y_size + i + 1] = (uint8_t)(128 + ((frame_index * 5) & 31));
    }
}

static AMediaFormat *new_video_format(const char *mime, int color_format) {
    AMediaFormat *format = AMediaFormat_new();
    if (!format) {
        return NULL;
    }
    AMediaFormat_setString(format, AMEDIAFORMAT_KEY_MIME, mime);
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_WIDTH, WIDTH);
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_HEIGHT, HEIGHT);
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_FRAME_RATE, FPS);
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_BIT_RATE, BITRATE);
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_I_FRAME_INTERVAL, 1);
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_COLOR_FORMAT, color_format);
    return format;
}

static int encode_h264(PacketList *packets, int *color_format_used) {
    const int color_formats[] = {COLOR_FORMAT_YUV420_SEMIPLANAR};
    AMediaCodec *codec = NULL;
    AMediaFormat *format = NULL;
    media_status_t status = AMEDIA_ERROR_UNKNOWN;

    for (size_t i = 0; i < sizeof(color_formats) / sizeof(color_formats[0]); i++) {
        codec = AMediaCodec_createEncoderByType("video/avc");
        format = new_video_format("video/avc", color_formats[i]);
        if (!codec || !format) {
            break;
        }
        status = AMediaCodec_configure(codec, format, NULL, NULL, AMEDIACODEC_CONFIGURE_FLAG_ENCODE);
        if (status == AMEDIA_OK) {
            *color_format_used = color_formats[i];
            break;
        }
        AMediaFormat_delete(format);
        AMediaCodec_delete(codec);
        format = NULL;
        codec = NULL;
    }

    if (!codec || !format || status != AMEDIA_OK) {
        fprintf(stderr, "encoder configure failed: %d\n", status);
        if (format) AMediaFormat_delete(format);
        if (codec) AMediaCodec_delete(codec);
        return -1;
    }

    if (AMediaCodec_start(codec) != AMEDIA_OK) {
        fprintf(stderr, "encoder start failed\n");
        AMediaFormat_delete(format);
        AMediaCodec_delete(codec);
        return -1;
    }

    int queued = 0;
    int eos_queued = 0;
    int saw_eos = 0;
    int spin = 0;
    const size_t frame_size = WIDTH * HEIGHT * 3 / 2;

    while (!saw_eos && spin++ < 10000) {
        if (!eos_queued) {
            ssize_t input_index = AMediaCodec_dequeueInputBuffer(codec, TIMEOUT_US);
            if (input_index >= 0) {
                size_t input_size = 0;
                uint8_t *input = AMediaCodec_getInputBuffer(codec, (size_t)input_index, &input_size);
                if (!input) {
                    fprintf(stderr, "encoder input buffer unavailable\n");
                    break;
                }
                if (queued < FRAME_COUNT) {
                    fill_nv12(input, input_size, queued);
                    int64_t pts_us = (int64_t)queued * 1000000 / FPS;
                    AMediaCodec_queueInputBuffer(codec, (size_t)input_index, 0, frame_size, pts_us, 0);
                    queued++;
                } else {
                    AMediaCodec_queueInputBuffer(codec, (size_t)input_index, 0, 0,
                                                 (int64_t)queued * 1000000 / FPS,
                                                 AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM);
                    eos_queued = 1;
                }
            }
        }

        AMediaCodecBufferInfo info;
        ssize_t output_index = AMediaCodec_dequeueOutputBuffer(codec, &info, TIMEOUT_US);
        if (output_index >= 0) {
            size_t output_size = 0;
            uint8_t *output = AMediaCodec_getOutputBuffer(codec, (size_t)output_index, &output_size);
            if (output && info.size > 0 && info.offset >= 0 &&
                (size_t)(info.offset + info.size) <= output_size) {
                if (push_packet(packets, output + info.offset, (size_t)info.size,
                                info.presentationTimeUs, info.flags) != 0) {
                    fprintf(stderr, "failed to store encoded packet\n");
                    AMediaCodec_releaseOutputBuffer(codec, (size_t)output_index, false);
                    break;
                }
            }
            if ((info.flags & AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM) != 0) {
                saw_eos = 1;
            }
            AMediaCodec_releaseOutputBuffer(codec, (size_t)output_index, false);
        } else if (output_index == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED) {
            AMediaFormat *out = AMediaCodec_getOutputFormat(codec);
            if (out) {
                fprintf(stderr, "encoder output format: %s\n", AMediaFormat_toString(out));
                AMediaFormat_delete(out);
            }
        }
    }

    AMediaCodec_stop(codec);
    AMediaFormat_delete(format);
    AMediaCodec_delete(codec);
    return saw_eos && queued == FRAME_COUNT && packets->len > 0 ? 0 : -1;
}

static int decode_h264(const PacketList *packets, int *decoded_frames, int *out_width, int *out_height) {
    AMediaCodec *codec = AMediaCodec_createDecoderByType("video/avc");
    AMediaFormat *format = AMediaFormat_new();
    if (!codec || !format) {
        fprintf(stderr, "decoder create failed\n");
        if (format) AMediaFormat_delete(format);
        if (codec) AMediaCodec_delete(codec);
        return -1;
    }
    AMediaFormat_setString(format, AMEDIAFORMAT_KEY_MIME, "video/avc");
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_WIDTH, WIDTH);
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_HEIGHT, HEIGHT);

    media_status_t status = AMediaCodec_configure(codec, format, NULL, NULL, 0);
    if (status != AMEDIA_OK) {
        fprintf(stderr, "decoder configure failed: %d\n", status);
        AMediaFormat_delete(format);
        AMediaCodec_delete(codec);
        return -1;
    }
    if (AMediaCodec_start(codec) != AMEDIA_OK) {
        fprintf(stderr, "decoder start failed\n");
        AMediaFormat_delete(format);
        AMediaCodec_delete(codec);
        return -1;
    }

    size_t next_packet = 0;
    int eos_queued = 0;
    int saw_eos = 0;
    int spin = 0;

    while (!saw_eos && spin++ < 10000) {
        if (!eos_queued) {
            ssize_t input_index = AMediaCodec_dequeueInputBuffer(codec, TIMEOUT_US);
            if (input_index >= 0) {
                size_t input_size = 0;
                uint8_t *input = AMediaCodec_getInputBuffer(codec, (size_t)input_index, &input_size);
                if (!input) {
                    fprintf(stderr, "decoder input buffer unavailable\n");
                    break;
                }
                if (next_packet < packets->len) {
                    const Packet *packet = &packets->items[next_packet++];
                    if (packet->size > input_size) {
                        fprintf(stderr, "decoder input too small: packet=%zu buffer=%zu\n",
                                packet->size, input_size);
                        break;
                    }
                    memcpy(input, packet->data, packet->size);
                    AMediaCodec_queueInputBuffer(codec, (size_t)input_index, 0, packet->size,
                                                 packet->pts_us, packet->flags);
                } else {
                    AMediaCodec_queueInputBuffer(codec, (size_t)input_index, 0, 0,
                                                 (int64_t)FRAME_COUNT * 1000000 / FPS,
                                                 AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM);
                    eos_queued = 1;
                }
            }
        }

        AMediaCodecBufferInfo info;
        ssize_t output_index = AMediaCodec_dequeueOutputBuffer(codec, &info, TIMEOUT_US);
        if (output_index >= 0) {
            if (info.size > 0) {
                *decoded_frames += 1;
            }
            if ((info.flags & AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM) != 0) {
                saw_eos = 1;
            }
            AMediaCodec_releaseOutputBuffer(codec, (size_t)output_index, false);
        } else if (output_index == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED) {
            AMediaFormat *out = AMediaCodec_getOutputFormat(codec);
            if (out) {
                int32_t width = 0;
                int32_t height = 0;
                AMediaFormat_getInt32(out, AMEDIAFORMAT_KEY_WIDTH, &width);
                AMediaFormat_getInt32(out, AMEDIAFORMAT_KEY_HEIGHT, &height);
                if (width > 0) *out_width = width;
                if (height > 0) *out_height = height;
                fprintf(stderr, "decoder output format: %s\n", AMediaFormat_toString(out));
                AMediaFormat_delete(out);
            }
        }
    }

    AMediaCodec_stop(codec);
    AMediaFormat_delete(format);
    AMediaCodec_delete(codec);
    return saw_eos && *decoded_frames == FRAME_COUNT ? 0 : -1;
}

int main(void) {
    PacketList packets = {0};
    int color_format = 0;
    int decoded_frames = 0;
    int decoded_width = WIDTH;
    int decoded_height = HEIGHT;

    int encode_ok = encode_h264(&packets, &color_format) == 0;
    int write_ok = encode_ok && write_packets("/data/local/tmp/video_hw_smoke.h264", &packets) == 0;
    int decode_ok = encode_ok && decode_h264(&packets, &decoded_frames, &decoded_width, &decoded_height) == 0;

    printf("{\"codec\":\"h264\",\"width\":%d,\"height\":%d,\"frames_in\":%d,"
           "\"encoded_packets\":%zu,\"encoded_bytes\":%zu,\"keyframes\":%d,"
           "\"codec_config_packets\":%d,\"color_format\":%d,"
           "\"decoded_frames\":%d,\"decoded_width\":%d,\"decoded_height\":%d,"
           "\"encode_ok\":%s,\"decode_ok\":%s,\"write_ok\":%s,\"status\":\"%s\"}\n",
           WIDTH, HEIGHT, FRAME_COUNT, packets.len, packets.total_bytes, packets.keyframes,
           packets.codec_config_count, color_format, decoded_frames, decoded_width, decoded_height,
           encode_ok ? "true" : "false", decode_ok ? "true" : "false",
           write_ok ? "true" : "false", (encode_ok && decode_ok && write_ok) ? "PASS" : "FAIL");

    free_packets(&packets);
    return (encode_ok && decode_ok && write_ok) ? 0 : 1;
}
