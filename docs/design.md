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
- requestor-side `TARGETS` and target data reads from an external owner
- XFixes owner-change notifications when the server supports XFixes
- X11 `INCR` receive/send for large target payloads
- hidden owner/requestor window naming for diagnostics and multi-product hosts

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
clipbus_clipboard_request_targets(clipboard, &request_id);
clipbus_clipboard_request_target_data(clipboard, "text/plain", max_bytes, &request_id);
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

Requestor flow:

```text
host requests current target list
  -> clipbus sends ConvertSelection(CLIPBOARD, TARGETS)
  -> external owner writes Atom list
  -> clipbus resolves Atom names
  -> clipbus_target_list_cb(user, request_id, status, targets, count)

host requests one target payload
  -> clipbus sends ConvertSelection(CLIPBOARD, target)
  -> external owner writes bytes or starts INCR
  -> clipbus reads bounded bytes
  -> clipbus_target_data_cb(user, request_id, target, status, data, len)
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
- request `TARGETS` from the current external owner
- request bounded target data from the current external owner
- receive external `INCR` target payloads
- send owner-side `INCR` target payloads
- notify `targets_changed` from XFixes selection owner events when available
- request timeout cleanup

Next slices:

- automated real X server integration smoke tests
- PRIMARY selection support if FusionDesk needs it
- Wayland/portal backend if Linux scope expands beyond X11

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
