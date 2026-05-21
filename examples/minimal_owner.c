#include <clipbus.h>

#include <stdint.h>
#include <stdio.h>

static clipbus_status_t on_target_request(
    void* user,
    clipbus_request_id_t request_id,
    const char* native_target,
    uint64_t max_inline_bytes,
    uint64_t timeout_ms)
{
    (void)user;
    (void)request_id;
    (void)native_target;
    (void)max_inline_bytes;
    (void)timeout_ms;
    return CLIPBUS_UNSUPPORTED;
}

int main(void)
{
    clipbus_options_t options = {
        .abi_version = CLIPBUS_ABI_VERSION,
        .backend = CLIPBUS_BACKEND_FAKE,
        .display_name = 0,
        .request_timeout_ms = 1000,
        .max_inline_bytes = 4096,
    };
    clipbus_callbacks_t callbacks = {
        .abi_version = CLIPBUS_ABI_VERSION,
        .target_request = on_target_request,
        .targets_changed = 0,
        .owner_lost = 0,
        .error = 0,
    };
    clipbus_clipboard_t* clipboard = 0;
    clipbus_status_t status =
        clipbus_clipboard_create(&options, &callbacks, 0, &clipboard);
    if (status != CLIPBUS_OK) {
        fprintf(stderr, "clipbus create failed: %s\n", clipbus_status_name(status));
        return 1;
    }
    status = clipbus_clipboard_start(clipboard);
    if (status != CLIPBUS_OK) {
        fprintf(stderr, "clipbus start failed: %s\n", clipbus_status_name(status));
        clipbus_clipboard_destroy(clipboard);
        return 1;
    }
    clipbus_clipboard_stop(clipboard);
    clipbus_clipboard_destroy(clipboard);
    return 0;
}

