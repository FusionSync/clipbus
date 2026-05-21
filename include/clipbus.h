#ifndef CLIPBUS_H
#define CLIPBUS_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32)
#define CLIPBUS_EXPORT __declspec(dllexport)
#else
#define CLIPBUS_EXPORT __attribute__((visibility("default")))
#endif

#define CLIPBUS_ABI_VERSION 1

typedef struct clipbus_clipboard clipbus_clipboard_t;
typedef uint64_t clipbus_request_id_t;

typedef enum clipbus_status {
    CLIPBUS_OK = 0,
    CLIPBUS_PENDING = 1,
    CLIPBUS_UNSUPPORTED = 2,
    CLIPBUS_TIMEOUT = 3,
    CLIPBUS_PLATFORM_ERROR = 4,
    CLIPBUS_INVALID_ARGUMENT = 5,
    CLIPBUS_ALREADY_STARTED = 6,
    CLIPBUS_NOT_STARTED = 7,
    CLIPBUS_NOT_FOUND = 8,
    CLIPBUS_PANIC = 9
} clipbus_status_t;

typedef enum clipbus_backend {
    CLIPBUS_BACKEND_AUTO = 0,
    CLIPBUS_BACKEND_X11 = 1,
    CLIPBUS_BACKEND_FAKE = 2
} clipbus_backend_t;

typedef struct clipbus_options {
    uint32_t abi_version;
    clipbus_backend_t backend;
    const char* display_name;
    uint64_t request_timeout_ms;
    uint64_t max_inline_bytes;
} clipbus_options_t;

typedef struct clipbus_target_offer {
    const char* native_target;
    uint8_t is_promised;
    uint8_t reserved0;
    uint16_t reserved1;
    uint64_t max_bytes;
} clipbus_target_offer_t;

typedef clipbus_status_t (*clipbus_target_request_cb)(
    void* user,
    clipbus_request_id_t request_id,
    const char* native_target,
    uint64_t max_inline_bytes,
    uint64_t timeout_ms);

typedef void (*clipbus_targets_changed_cb)(void* user);
typedef void (*clipbus_owner_lost_cb)(void* user);
typedef void (*clipbus_error_cb)(void* user, int code, const char* message);

typedef struct clipbus_callbacks {
    uint32_t abi_version;
    clipbus_target_request_cb target_request;
    clipbus_targets_changed_cb targets_changed;
    clipbus_owner_lost_cb owner_lost;
    clipbus_error_cb error;
} clipbus_callbacks_t;

CLIPBUS_EXPORT clipbus_status_t clipbus_clipboard_create(
    const clipbus_options_t* options,
    const clipbus_callbacks_t* callbacks,
    void* user,
    clipbus_clipboard_t** out);

CLIPBUS_EXPORT clipbus_status_t clipbus_clipboard_start(
    clipbus_clipboard_t* clipboard);

CLIPBUS_EXPORT clipbus_status_t clipbus_clipboard_stop(
    clipbus_clipboard_t* clipboard);

CLIPBUS_EXPORT void clipbus_clipboard_destroy(clipbus_clipboard_t* clipboard);

CLIPBUS_EXPORT clipbus_status_t clipbus_clipboard_publish_targets(
    clipbus_clipboard_t* clipboard,
    const clipbus_target_offer_t* targets,
    size_t target_count);

CLIPBUS_EXPORT clipbus_status_t clipbus_clipboard_clear(
    clipbus_clipboard_t* clipboard);

CLIPBUS_EXPORT clipbus_status_t clipbus_clipboard_complete_request(
    clipbus_clipboard_t* clipboard,
    clipbus_request_id_t request_id,
    clipbus_status_t status,
    const uint8_t* data,
    size_t data_len);

CLIPBUS_EXPORT const char* clipbus_status_name(clipbus_status_t status);

#ifdef __cplusplus
}
#endif

#endif
