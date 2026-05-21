#define _POSIX_C_SOURCE 199309L

#include <clipbus.h>

#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static const uint8_t k_payload[] =
    "clipbus x11 self smoke payload that is intentionally longer than eight bytes";

struct smoke_context {
    clipbus_clipboard_t* clipboard;
    atomic_int saw_targets;
    atomic_int saw_data;
    atomic_int saw_error;
    char error[256];
};

static void sleep_ms(long ms)
{
    struct timespec ts = {
        .tv_sec = ms / 1000,
        .tv_nsec = (ms % 1000) * 1000000L,
    };
    nanosleep(&ts, 0);
}

static int wait_flag(atomic_int* flag)
{
    for (int i = 0; i < 200; ++i) {
        if (atomic_load(flag) != 0) {
            return 1;
        }
        sleep_ms(10);
    }
    return 0;
}

static clipbus_status_t on_target_request(
    void* user,
    clipbus_request_id_t request_id,
    const char* native_target,
    uint64_t max_inline_bytes,
    uint64_t timeout_ms)
{
    (void)max_inline_bytes;
    (void)timeout_ms;

    struct smoke_context* context = (struct smoke_context*)user;
    if (strcmp(native_target, "text/plain") != 0) {
        return CLIPBUS_UNSUPPORTED;
    }
    clipbus_status_t status = clipbus_clipboard_complete_request(
        context->clipboard,
        request_id,
        CLIPBUS_OK,
        k_payload,
        sizeof(k_payload) - 1);
    return status == CLIPBUS_OK ? CLIPBUS_PENDING : status;
}

static void on_target_list(
    void* user,
    clipbus_request_id_t request_id,
    clipbus_status_t status,
    const char* const* native_targets,
    size_t target_count)
{
    (void)request_id;

    struct smoke_context* context = (struct smoke_context*)user;
    if (status != CLIPBUS_OK) {
        atomic_store(&context->saw_error, 1);
        return;
    }
    for (size_t i = 0; i < target_count; ++i) {
        if (strcmp(native_targets[i], "text/plain") == 0) {
            atomic_store(&context->saw_targets, 1);
            return;
        }
    }
}

static void on_target_data(
    void* user,
    clipbus_request_id_t request_id,
    const char* native_target,
    clipbus_status_t status,
    const uint8_t* data,
    size_t data_len)
{
    (void)request_id;

    struct smoke_context* context = (struct smoke_context*)user;
    if (status == CLIPBUS_OK && strcmp(native_target, "text/plain") == 0 &&
        data_len == sizeof(k_payload) - 1 && memcmp(data, k_payload, data_len) == 0) {
        atomic_store(&context->saw_data, 1);
    } else {
        atomic_store(&context->saw_error, 1);
    }
}

static void on_error(void* user, int code, const char* message)
{
    struct smoke_context* context = (struct smoke_context*)user;
    snprintf(context->error, sizeof(context->error), "%d:%s", code, message);
    atomic_store(&context->saw_error, 1);
}

int main(void)
{
    if (getenv("DISPLAY") == 0) {
        fprintf(stderr, "DISPLAY is not set\n");
        return 77;
    }

    struct smoke_context context;
    memset(&context, 0, sizeof(context));

    clipbus_options_t options = {
        .abi_version = CLIPBUS_ABI_VERSION,
        .backend = CLIPBUS_BACKEND_X11,
        .display_name = 0,
        .request_timeout_ms = 2000,
        .max_inline_bytes = 8,
    };
    clipbus_callbacks_t callbacks = {
        .abi_version = CLIPBUS_ABI_VERSION,
        .target_request = on_target_request,
        .targets_changed = 0,
        .owner_lost = 0,
        .error = on_error,
        .target_list = on_target_list,
        .target_data = on_target_data,
    };

    clipbus_status_t status =
        clipbus_clipboard_create(&options, &callbacks, &context, &context.clipboard);
    if (status != CLIPBUS_OK) {
        fprintf(stderr, "create failed: %s\n", clipbus_status_name(status));
        return 1;
    }
    status = clipbus_clipboard_start(context.clipboard);
    if (status != CLIPBUS_OK) {
        fprintf(stderr, "start failed: %s\n", clipbus_status_name(status));
        clipbus_clipboard_destroy(context.clipboard);
        return 1;
    }

    clipbus_target_offer_t offer = {
        .native_target = "text/plain",
        .is_promised = 1,
        .reserved0 = 0,
        .reserved1 = 0,
        .max_bytes = 4096,
    };
    status = clipbus_clipboard_publish_targets(context.clipboard, &offer, 1);
    if (status != CLIPBUS_OK) {
        fprintf(stderr, "publish failed: %s\n", clipbus_status_name(status));
        clipbus_clipboard_destroy(context.clipboard);
        return 1;
    }

    clipbus_request_id_t request_id = 0;
    status = clipbus_clipboard_request_targets(context.clipboard, &request_id);
    if (status != CLIPBUS_OK || !wait_flag(&context.saw_targets)) {
        fprintf(stderr, "target list failed: %s\n", clipbus_status_name(status));
        clipbus_clipboard_destroy(context.clipboard);
        return 1;
    }

    status = clipbus_clipboard_request_target_data(
        context.clipboard,
        "text/plain",
        4096,
        &request_id);
    if (status != CLIPBUS_OK || !wait_flag(&context.saw_data)) {
        fprintf(
            stderr,
            "target data failed: %s %s\n",
            clipbus_status_name(status),
            context.error);
        clipbus_clipboard_destroy(context.clipboard);
        return 1;
    }

    clipbus_clipboard_stop(context.clipboard);
    clipbus_clipboard_destroy(context.clipboard);
    return 0;
}
