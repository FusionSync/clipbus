# clipbus Design

## Goal

`clipbus` wraps native clipboard event mechanics behind a small C ABI. The
first backend is X11/XCB. The host process owns application semantics and data
retrieval; `clipbus` owns native selection events.

## Boundaries

`clipbus` owns:

- X11 connection and hidden owner/requestor window
- `CLIPBOARD` selection ownership
- `TARGETS`, `TIMESTAMP`, and `SAVE_TARGETS` responses
- `SelectionRequest`, `SelectionNotify`, `SelectionClear`, and property writes
- pending request tracking and request timeout failure

The host owns:

- protocol encoding
- policy and audit
- session and reconnect
- remote file object identity
- remote byte reads
- filesystem promise lifecycle

## C ABI Shape

The ABI is handle based:

```c
clipbus_clipboard_t* clipboard;
clipbus_clipboard_create(&options, &callbacks, user, &clipboard);
clipbus_clipboard_start(clipboard);
clipbus_clipboard_publish_targets(clipboard, targets, target_count);
clipbus_clipboard_complete_request(clipboard, request_id, status, data, len);
clipbus_clipboard_stop(clipboard);
clipbus_clipboard_destroy(clipboard);
```

Callback flow:

```text
local X11 app requests a target
  -> clipbus records request_id
  -> clipbus_target_request_cb(user, request_id, target, max_bytes, timeout)
  -> host returns Pending/Unsupported or later completes request_id
  -> clipbus writes the X11 property and sends SelectionNotify
```

## Current Slice

Implemented in the first slice:

- library crate, staticlib, cdylib, and rlib outputs
- public C header
- fake backend for lifecycle tests
- X11 backend thread
- publish target list and own `CLIPBOARD`
- respond to `TARGETS`, `TIMESTAMP`, and `SAVE_TARGETS`
- promise-style target request callback
- complete or fail pending X11 requests
- request timeout cleanup

Next slices:

- XFixes owner-change monitor for external clipboard changes
- local target list snapshot from an external owner
- local target read requests from an external owner
- INCR for large inline payloads
- better install/export targets after CMake is available in CI

## Packaging

The primary install path follows the `fuse-promise` model: Cargo builds the
library, and `scripts/install-dev.sh` stages the public ABI payload.

Installed files:

```text
<includedir>/clipbus.h
<libdir>/libclipbus.so.<version>
<libdir>/libclipbus.so.<soname-major>
<libdir>/libclipbus.so
<libdir>/libclipbus.a
<libdir>/pkgconfig/clipbus.pc
<libdir>/cmake/Clipbus/ClipbusConfig.cmake
```

Packagers should use `DESTDIR` while keeping generated metadata rooted at the
final prefix:

```sh
DESTDIR="$pkgdir" PREFIX=/usr BUILD_PROFILE=release SONAME_MAJOR=0 scripts/install-dev.sh
```

The install metadata gate is:

```sh
tests/install-metadata.sh
```

It verifies both public consumption paths:

```sh
pkg-config --cflags --libs clipbus
find_package(Clipbus REQUIRED)
```
