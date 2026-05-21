# clipbus

`clipbus` is a small clipboard event library for Linux/X11.

It owns the native X11 selection event loop and exposes a stable C ABI to host
applications. Data is delivered through promise-style callbacks: the library
advertises targets, records local paste requests, and waits for the host to
complete each request with bytes or an error.

## Scope

Current scope:

- X11 `CLIPBOARD` ownership through XCB/X11 protocol bindings
- `TARGETS` response
- external owner `TARGETS` reads
- bounded external target data reads
- delayed target rendering through `clipbus_target_request_cb`
- X11 `INCR` receive/send for large target payloads
- XFixes owner-change notification when available
- owner-loss and diagnostics callbacks

Out of scope for this library:

- application protocol semantics
- remote file range reads
- filesystem promises
- policy and audit
- Wayland or portal integration

## ABI

The public C ABI is declared in `include/clipbus.h`.

```c
clipbus_clipboard_t* clipboard = NULL;
clipbus_clipboard_create(&options, &callbacks, user, &clipboard);
clipbus_clipboard_start(clipboard);
clipbus_clipboard_publish_targets(clipboard, targets, target_count);
clipbus_clipboard_request_targets(clipboard, &request_id);
clipbus_clipboard_request_target_data(clipboard, "text/plain", max_bytes, &request_id);
clipbus_clipboard_complete_request(clipboard, request_id, CLIPBUS_OK, data, len);
clipbus_clipboard_stop(clipboard);
clipbus_clipboard_destroy(clipboard);
```

Callbacks must be bounded. They may return `CLIPBUS_PENDING`; the host then
completes the request later with `clipbus_clipboard_complete_request`.

See `docs/design.md` for the implementation boundary and slice plan.

## Build And Install

```sh
cargo build --locked
cargo test --locked
BUILD_PROFILE=release PREFIX=/usr/local scripts/install-dev.sh
```

The installed public boundary is:

```sh
pkg-config --cflags --libs clipbus
```

CMake consumers may also use:

```cmake
find_package(Clipbus REQUIRED)
target_link_libraries(app PRIVATE Clipbus::Clipbus)
```

For a live X11 owner/requestor smoke, build `examples/x11_self_smoke.c` against
an installed or staged `libclipbus`. The example publishes `text/plain`, reads
it back through the requestor API, and forces the payload through X11 `INCR`.
