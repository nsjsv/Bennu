/*
 * 用系统的 libjpeg-turbo 逐行解 JPEG：超大图切块时只占几行的内存（纯 Rust 的解码器只能整张解）。
 *
 * libjpeg 出错时默认调 exit()。这里换成 longjmp 回到本文件的入口函数，转成错误文字返回；
 * setjmp/longjmp 只在本文件和 libjpeg 的 C 帧之间跳，不穿过 Rust（本项目 panic = abort，
 * 遇错就 panic 的解码库会让坏图把程序带崩）。
 */
#include <errno.h>
#include <setjmp.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <jpeglib.h>

#ifndef JCS_ALPHA_EXTENSIONS
#error "libjpeg-turbo with JCS_EXT_RGBA (1.2.1 or later) is required"
#endif

enum {
    BENNU_JPEG_OK = 0,
    BENNU_JPEG_FAILED = 1,
    /* CMYK/YCCK：libjpeg 不转 RGB，交回整张解码（image 库会转）。 */
    BENNU_JPEG_UNSUPPORTED = 2,
};

struct bennu_jpeg_error {
    struct jpeg_error_mgr base;
    jmp_buf jump;
    char message[JMSG_LENGTH_MAX];
};

struct bennu_jpeg {
    struct jpeg_decompress_struct decompress;
    struct bennu_jpeg_error error;
    FILE *file;
};

static void bennu_error_exit(j_common_ptr info) {
    struct bennu_jpeg_error *error = (struct bennu_jpeg_error *)info->err;
    (*info->err->format_message)(info, error->message);
    longjmp(error->jump, 1);
}

/* 数据提前结束这类警告不打印到终端：能解出多少显示多少，和常见看图软件一样。 */
static void bennu_output_message(j_common_ptr info) { (void)info; }

static void copy_message(char *out, size_t out_len, const char *message) {
    if (out_len == 0) {
        return;
    }
    strncpy(out, message, out_len - 1);
    out[out_len - 1] = '\0';
}

static void bennu_jpeg_free(struct bennu_jpeg *jpeg) {
    jpeg_destroy_decompress(&jpeg->decompress);
    if (jpeg->file != NULL) {
        fclose(jpeg->file);
    }
    free(jpeg);
}

/* 打开并读完文件头，开始解码；输出每像素 4 字节 RGBA（灰度也转成 RGBA）。 */
int bennu_jpeg_open(const char *path, struct bennu_jpeg **out, unsigned *width, unsigned *height,
                    char *message, size_t message_len) {
    struct bennu_jpeg *jpeg = calloc(1, sizeof(*jpeg));
    if (jpeg == NULL) {
        copy_message(message, message_len, "out of memory");
        return BENNU_JPEG_FAILED;
    }
    jpeg->file = fopen(path, "rb");
    if (jpeg->file == NULL) {
        copy_message(message, message_len, strerror(errno));
        free(jpeg);
        return BENNU_JPEG_FAILED;
    }
    jpeg->decompress.err = jpeg_std_error(&jpeg->error.base);
    jpeg->error.base.error_exit = bennu_error_exit;
    jpeg->error.base.output_message = bennu_output_message;
    if (setjmp(jpeg->error.jump)) {
        copy_message(message, message_len, jpeg->error.message);
        bennu_jpeg_free(jpeg);
        return BENNU_JPEG_FAILED;
    }
    jpeg_create_decompress(&jpeg->decompress);
    jpeg_stdio_src(&jpeg->decompress, jpeg->file);
    jpeg_read_header(&jpeg->decompress, TRUE);
    if (jpeg->decompress.jpeg_color_space == JCS_CMYK
        || jpeg->decompress.jpeg_color_space == JCS_YCCK) {
        copy_message(message, message_len, "CMYK JPEG");
        bennu_jpeg_free(jpeg);
        return BENNU_JPEG_UNSUPPORTED;
    }
    jpeg->decompress.out_color_space = JCS_EXT_RGBA;
    jpeg_start_decompress(&jpeg->decompress);
    *width = jpeg->decompress.output_width;
    *height = jpeg->decompress.output_height;
    *out = jpeg;
    return BENNU_JPEG_OK;
}

/* 接着往下解，最多 `max_rows` 行写进 `rgba`（每行宽×4 字节，行紧挨着）。读完了 `*rows_read` 为 0。
 * 出错后这个对象不能再用，只能关掉。 */
int bennu_jpeg_read_rows(struct bennu_jpeg *jpeg, unsigned char *rgba, unsigned max_rows,
                         unsigned *rows_read, char *message, size_t message_len) {
    struct jpeg_decompress_struct *decompress = &jpeg->decompress;
    unsigned rows = 0;
    size_t stride = (size_t)decompress->output_width * 4;
    if (setjmp(jpeg->error.jump)) {
        copy_message(message, message_len, jpeg->error.message);
        return BENNU_JPEG_FAILED;
    }
    while (rows < max_rows && decompress->output_scanline < decompress->output_height) {
        JSAMPROW row = rgba + (size_t)rows * stride;
        rows += jpeg_read_scanlines(decompress, &row, 1);
    }
    *rows_read = rows;
    return BENNU_JPEG_OK;
}

void bennu_jpeg_close(struct bennu_jpeg *jpeg) {
    if (jpeg != NULL) {
        bennu_jpeg_free(jpeg);
    }
}
